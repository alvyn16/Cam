//! The Glide app: one main window (Home ↔ Editor), a floating recording
//! widget, a menu bar item and a global hotkey (⌥⇧⌘R). egui on wgpu with
//! a transparent titlebar and Inter, sharing one GPU device with the
//! editor's renderer.

pub mod editor;
pub mod home;
pub mod hotkey;
pub mod inspector;
pub mod macos;
pub mod menubar;
pub mod player;
pub mod preview;
pub mod snapshot;
pub mod theme;
pub mod timeline;
pub mod widgets;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{Color32, RichText, Sense, vec2};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId, WindowLevel};

use crate::recorder::{self, Recorder, Target};
use crate::render::Gpu;
use editor::Editor;
use home::{Devices, Settings, SourceKind, Thumbs};

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    ToggleRecording,
    TogglePause,
    Restart,
    Cancel,
    ShowWindow,
    OpenFolder,
    Open(PathBuf),
    Quit,
}

static QUEUE: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());
static PROXY: OnceLock<Mutex<EventLoopProxy<()>>> = OnceLock::new();
static DRAG: AtomicBool = AtomicBool::new(false);

fn wake() {
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap().send_event(());
    }
}

pub fn push(c: Cmd) {
    QUEUE.lock().unwrap().push(c);
    wake();
}

/// Empty toolbar space drags the window (the titlebar is transparent).
pub fn toolbar_drag(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let bg = ui.interact(rect, ui.id().with("drag"), Sense::click_and_drag());
    if bg.drag_started() {
        DRAG.store(true, Ordering::Relaxed);
    }
    if bg.double_clicked() {
        push(Cmd::ShowWindow);
    }
}

struct Win {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    egui: egui_winit::State,
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    repaint_at: Option<Instant>,
}

enum View {
    Home,
    Editor(Box<Editor>),
}

struct Recording {
    rec: Option<Recorder>,
    /// Countdown before capture starts.
    countdown_until: Option<Instant>,
}

type Finished = Result<PathBuf, String>;

/// Scripted run for `GLIDE_GUI_SELFTEST=1` (scripts/selftest.sh): record a
/// test pattern, stop, open it in the editor, export, check and quit. Never
/// touches the mouse or needs permissions.
struct SelfTest {
    phase: u8,
    at: Instant,
    paused_at: Option<Instant>,
    resumed: bool,
}

fn log(msg: &str) {
    eprintln!("selftest: {msg}");
}

struct App {
    gpu: Option<Gpu>,
    main: Option<Win>,
    widget: Option<Win>,
    view: View,
    settings: Settings,
    devices: Devices,
    projects: Vec<home::ProjectEntry>,
    thumbs: Thumbs,
    recording: Option<Recording>,
    finishing: bool,
    finished: Arc<Mutex<Option<Finished>>>,
    error: Option<String>,
    menubar: Option<menubar::MenuBar>,
    last_tick: Instant,
    open_at_launch: Option<PathBuf>,
    permission_note: Option<String>,
    selftest: Option<SelfTest>,
}

fn permission_note() -> Option<String> {
    use crate::capture::tcc::{self, Status};
    if tcc::screen_status() != Status::Granted {
        return Some("Glide needs Screen Recording access. Request it, then enable Glide in System Settings › Privacy & Security › Screen & System Audio Recording, and reopen Glide.".into());
    }
    None
}

impl App {
    fn new(open: Option<PathBuf>) -> App {
        let selftest = std::env::var_os("GLIDE_GUI_SELFTEST").map(|_| SelfTest { phase: 0, at: Instant::now(), paused_at: None, resumed: false });
        let mut settings = if selftest.is_some() { Settings::default() } else { Settings::load() };
        if let Some(dir) = std::env::var_os("GLIDE_OUTPUT_DIR") {
            settings.projects_dir = Some(PathBuf::from(dir));
        }
        if selftest.is_some() {
            settings.countdown = false;
        }
        let projects = home::scan_projects(&settings.dir());
        App {
            gpu: None,
            main: None,
            widget: None,
            view: View::Home,
            devices: Devices::load(),
            settings,
            projects,
            thumbs: Thumbs::default(),
            recording: None,
            finishing: false,
            finished: Arc::new(Mutex::new(None)),
            error: None,
            menubar: None,
            last_tick: Instant::now(),
            open_at_launch: open,
            permission_note: permission_note(),
            selftest,
        }
    }

    fn open_window(&mut self, el: &ActiveEventLoop, title: &str, size: (f64, f64), min: (f64, f64), floating: bool) -> Win {
        use winit::platform::macos::WindowAttributesExtMacOS;
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(winit::dpi::LogicalSize::new(size.0, size.1))
            .with_min_inner_size(winit::dpi::LogicalSize::new(min.0, min.1))
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true);
        if floating {
            attrs = attrs.with_window_level(WindowLevel::AlwaysOnTop).with_resizable(false);
        }
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        let surface = match &self.gpu {
            Some(g) => g.instance.create_surface(window.clone()).expect("surface"),
            None => {
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
                let s = instance.create_surface(window.clone()).expect("surface");
                let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&s),
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    ..Default::default()
                }))
                .expect("no compatible GPU adapter");
                let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("glide"),
                    required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                    ..Default::default()
                }))
                .expect("create device");
                self.gpu = Some(Gpu { instance, adapter, device, queue });
                s
            }
        };
        let g = self.gpu.as_ref().unwrap();
        let caps = surface.get_capabilities(&g.adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let px = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: px.width.max(1),
            height: px.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(&g.device, &config);
        let renderer = egui_wgpu::Renderer::new(&g.device, format, egui_wgpu::RendererOptions::default());
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(g.device.limits().max_texture_dimension_2d as usize),
        );
        let (x, y) = theme::TRAFFIC_LIGHTS;
        macos::inset_traffic_lights(&window, x, y);
        window.request_redraw();
        Win { window, surface, config, egui, ctx, renderer, repaint_at: None }
    }

    fn show_main(&mut self, el: &ActiveEventLoop) {
        if self.main.is_none() {
            let w = self.open_window(el, "Glide", (1320.0, 840.0), (980.0, 640.0), false);
            self.main = Some(w);
        }
        if let Some(w) = &self.main {
            w.window.set_visible(true);
            w.window.focus_window();
            w.window.request_redraw();
        }
        if self.selftest.is_none() {
            macos::activate();
        }
    }

    fn open_project(&mut self, el: &ActiveEventLoop, path: PathBuf) {
        self.show_main(el);
        let Some(gpu) = self.gpu.clone() else { return };
        match Editor::open(&path, gpu) {
            Ok(ed) => {
                if let Some(w) = &self.main {
                    w.window.set_title(&ed.project.name);
                }
                self.view = View::Editor(Box::new(ed));
            }
            Err(e) => self.error = Some(format!("Couldn't open {}: {e:#}", path.display())),
        }
    }

    fn recorder_options(&self) -> recorder::Options {
        let s = &self.settings;
        if self.selftest.is_some() {
            return recorder::Options { target: Target::Pattern, system_audio: false, mic: None, camera: None, fps: 60, keys: false, dir: s.dir(), name: Some("Selftest".into()) };
        }
        let target = match s.source {
            SourceKind::Display => Target::Display { id: s.display },
            SourceKind::Window => match s.window {
                Some(id) => Target::Window { id },
                None => Target::Display { id: s.display },
            },
            SourceKind::Area => Target::Area {
                display: s.display,
                rect: crate::engine::geom::Rect::new(s.area[0], s.area[1], s.area[2], s.area[3]),
            },
        };
        recorder::Options {
            target,
            system_audio: s.system_audio,
            mic: s.mic.then(|| s.mic_id.clone()),
            camera: s.camera.then(|| s.camera_id.clone()),
            fps: s.fps,
            keys: s.keys,
            dir: s.dir(),
            name: None,
        }
    }

    fn start_recording(&mut self, el: &ActiveEventLoop) {
        if self.recording.is_some() || self.finishing {
            return;
        }
        self.error = None;
        if self.selftest.is_none() {
            self.settings.save();
        }
        if let View::Editor(ed) = &mut self.view {
            ed.save();
        }
        let countdown = self.settings.countdown.then(|| Instant::now() + Duration::from_secs(3));
        self.recording = Some(Recording { rec: None, countdown_until: countdown });
        if let Some(w) = &self.main {
            w.window.set_visible(false);
        }
        if self.widget.is_none() {
            let w = self.open_window(el, "Glide Recording", (300.0, 64.0), (300.0, 64.0), true);
            self.widget = Some(w);
        }
        if countdown.is_none() {
            self.begin_capture();
        }
    }

    fn begin_capture(&mut self) {
        let opts = self.recorder_options();
        match Recorder::start(opts) {
            Ok(r) => {
                if let Some(rec) = &mut self.recording {
                    rec.rec = Some(r);
                    rec.countdown_until = None;
                }
            }
            Err(e) => {
                self.recording = None;
                self.error = Some(format!("Couldn't start recording: {e:#}"));
                self.widget = None;
                self.permission_note = permission_note();
                push(Cmd::ShowWindow);
            }
        }
    }

    fn stop_recording(&mut self) {
        let Some(rec) = self.recording.take() else { return };
        self.widget = None;
        let Some(r) = rec.rec else {
            push(Cmd::ShowWindow);
            return;
        };
        self.finishing = true;
        let done = self.finished.clone();
        std::thread::spawn(move || {
            let res = objc2::rc::autoreleasepool(|_| r.stop()).map_err(|e| format!("{e:#}"));
            *done.lock().unwrap() = Some(res);
            wake();
        });
    }

    fn cancel_recording(&mut self) {
        if let Some(rec) = self.recording.take()
            && let Some(r) = rec.rec
        {
            std::thread::spawn(move || objc2::rc::autoreleasepool(|_| r.cancel()));
        }
        self.widget = None;
        push(Cmd::ShowWindow);
    }

    fn handle(&mut self, el: &ActiveEventLoop, c: Cmd) {
        match c {
            Cmd::ToggleRecording => {
                if self.recording.is_some() {
                    self.stop_recording();
                } else {
                    self.start_recording(el);
                }
            }
            Cmd::TogglePause => {
                if let Some(Recording { rec: Some(r), .. }) = &mut self.recording {
                    if r.is_paused() { r.resume() } else { r.pause() }
                }
            }
            Cmd::Restart => {
                self.cancel_recording();
                self.start_recording(el);
            }
            Cmd::Cancel => self.cancel_recording(),
            Cmd::ShowWindow => self.show_main(el),
            Cmd::OpenFolder => macos::open_folder(&self.settings.dir()),
            Cmd::Open(p) => self.open_project(el, p),
            Cmd::Quit => {
                if let View::Editor(ed) = &mut self.view {
                    ed.save();
                }
                if self.recording.is_some() {
                    self.stop_recording();
                }
                while self.finishing && self.finished.lock().unwrap().is_none() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                el.exit();
            }
        }
    }

    fn tick(&mut self, el: &ActiveEventLoop) {
        let cmds: Vec<Cmd> = std::mem::take(&mut *QUEUE.lock().unwrap());
        for c in cmds {
            self.handle(el, c);
        }
        if let Some(rec) = &self.recording
            && rec.rec.is_none()
            && rec.countdown_until.is_some_and(|t| Instant::now() >= t)
        {
            self.begin_capture();
        }
        if let Some(Recording { rec: Some(r), .. }) = &self.recording
            && let Some(e) = r.error()
        {
            self.error = Some(format!("Recording stopped: {e}"));
            self.stop_recording();
        }
        let done = self.finished.lock().unwrap().take();
        if let Some(res) = done {
            self.finishing = false;
            match res {
                Ok(path) => {
                    self.projects = home::scan_projects(&self.settings.dir());
                    self.open_project(el, path);
                }
                Err(e) => {
                    self.error = Some(format!("Recording failed: {e}"));
                    self.show_main(el);
                }
            }
        }
        self.selftest_step(el);
        let clock = match &self.recording {
            Some(Recording { rec: Some(r), .. }) => crate::capture::record::clock(r.elapsed()),
            Some(Recording { countdown_until: Some(t), .. }) => format!("{}", t.saturating_duration_since(Instant::now()).as_secs() + 1),
            _ => String::new(),
        };
        let paused = matches!(&self.recording, Some(Recording { rec: Some(r), .. }) if r.is_paused());
        if let Some(m) = &mut self.menubar {
            m.update(&menubar::View { recording: self.recording.is_some(), paused, finishing: self.finishing, clock: &clock });
        }
    }

    fn selftest_step(&mut self, el: &ActiveEventLoop) {
        let Some(st) = &mut self.selftest else { return };
        let since = st.at.elapsed();
        let fail = |msg: String| {
            log(&format!("FAIL {msg}"));
            std::process::exit(1);
        };
        if since > Duration::from_secs(120) {
            fail(format!("timed out in phase {}", st.phase));
        }
        match st.phase {
            0 if since > Duration::from_millis(800) => {
                log(&format!("launched main_window={} menubar={}", self.main.is_some(), self.menubar.is_some()));
                st.phase = 1;
                st.at = Instant::now();
                push(Cmd::ToggleRecording);
            }
            1 => {
                if let Some(Recording { rec: Some(r), .. }) = &self.recording {
                    match st.paused_at {
                        None if r.elapsed() >= 1.0 => {
                            st.paused_at = Some(Instant::now());
                            push(Cmd::TogglePause);
                        }
                        Some(t) if !st.resumed && t.elapsed() > Duration::from_millis(700) => {
                            log(&format!("paused={}", r.is_paused()));
                            if !r.is_paused() {
                                fail("pause had no effect".into());
                            }
                            st.resumed = true;
                            push(Cmd::TogglePause);
                        }
                        _ => {}
                    }
                    if st.resumed && r.elapsed() >= 3.0 {
                        log(&format!("recording widget={} elapsed={:.1}s", self.widget.is_some(), r.elapsed()));
                        st.phase = 2;
                        push(Cmd::ToggleRecording);
                    }
                }
                if let Some(e) = &self.error {
                    fail(e.clone());
                }
            }
            2 => {
                if let Some(e) = &self.error {
                    fail(e.clone());
                }
                if let View::Editor(ed) = &mut self.view {
                    log(&format!(
                        "editor opened {} duration={:.0}ms zooms={} mouse_events={}",
                        ed.package.root.display(),
                        ed.duration(),
                        ed.project.timeline.zooms.len(),
                        ed.rec.mouse.len()
                    ));
                    ed.start_export();
                    st.phase = 3;
                }
            }
            3 => {
                if let View::Editor(ed) = &self.view
                    && let Some(res) = &ed.export.result
                {
                    match res {
                        Ok((path, stats)) => {
                            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                            log(&format!("exported {} bytes={size} {stats:?}", path.display()));
                            if size == 0 {
                                fail("empty export".into());
                            }
                            log("PASS");
                            st.phase = 4;
                            push(Cmd::Quit);
                        }
                        Err(e) => fail(format!("export: {e}")),
                    }
                }
            }
            _ => {}
        }
        let _ = el;
    }

    fn draw_main(&mut self, ui: &mut egui::Ui, renderer: &mut egui_wgpu::Renderer) {
        if let Some(e) = self.error.clone() {
            egui::Panel::bottom("error").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(255, 69, 58), &e);
                    if ui.small_button("Dismiss").clicked() {
                        self.error = None;
                    }
                });
            });
        }
        if self.finishing {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Saving recording…").size(16.0));
                });
            });
            return;
        }
        match &mut self.view {
            View::Home => {
                let note = self.permission_note.clone();
                let action = home::ui(ui, &mut self.settings, &mut self.devices, &mut self.projects, &mut self.thumbs, note.as_deref());
                match action {
                    home::Action::Record => push(Cmd::ToggleRecording),
                    home::Action::Open(p) => push(Cmd::Open(p)),
                    home::Action::RequestPermissions => {
                        std::thread::spawn(|| crate::capture::tcc::request_all(true, false, false, Duration::from_secs(60)));
                    }
                    home::Action::None => {}
                }
            }
            View::Editor(ed) => {
                let mut back = false;
                ed.ui(ui, renderer, &mut back);
                if back {
                    self.view = View::Home;
                    self.projects = home::scan_projects(&self.settings.dir());
                    if let Some(w) = &self.main {
                        w.window.set_title("Glide");
                    }
                }
            }
        }
    }

    fn draw_widget(&mut self, ui: &mut egui::Ui) {
        let p = theme::pal(ui);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.window).inner_margin(egui::Margin { left: 76, right: 10, top: 0, bottom: 0 })).show(ui, |ui| {
            toolbar_drag(ui);
            ui.horizontal_centered(|ui| {
                match &self.recording {
                    Some(Recording { rec: None, countdown_until: Some(t), .. }) => {
                        let n = t.saturating_duration_since(Instant::now()).as_secs() + 1;
                        ui.label(RichText::new(format!("Starting in {n}…")).font(theme::semibold(14.0)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Cancel").clicked() {
                                push(Cmd::Cancel);
                            }
                        });
                    }
                    Some(Recording { rec: Some(r), .. }) => {
                        let paused = r.is_paused();
                        let dot = if paused { Color32::from_rgb(255, 159, 10) } else { Color32::from_rgb(255, 59, 48) };
                        let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(rect.center(), 5.0, dot);
                        ui.label(RichText::new(crate::capture::record::clock(r.elapsed())).font(theme::semibold(14.0)).monospace());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::record_button(ui, 26.0, true).on_hover_text("Stop (⌥⇧⌘R)").clicked() {
                                push(Cmd::ToggleRecording);
                            }
                            if widgets::icon_button(ui, if paused { widgets::Icon::Play } else { widgets::Icon::Pause }, true).on_hover_text(if paused { "Resume" } else { "Pause" }).clicked() {
                                push(Cmd::TogglePause);
                            }
                            if widgets::icon_button(ui, widgets::Icon::Restart, true).on_hover_text("Restart").clicked() {
                                push(Cmd::Restart);
                            }
                            if widgets::icon_button(ui, widgets::Icon::Close, true).on_hover_text("Cancel").clicked() {
                                push(Cmd::Cancel);
                            }
                        });
                    }
                    _ => {
                        ui.label("Starting…");
                    }
                }
            });
        });
    }

    fn redraw(&mut self, which: WindowId) {
        let is_main = self.main.as_ref().is_some_and(|w| w.window.id() == which);
        let is_widget = self.widget.as_ref().is_some_and(|w| w.window.id() == which);
        if !is_main && !is_widget {
            return;
        }
        let Some(gpu) = self.gpu.clone() else { return };
        let mut win = if is_main { self.main.take().unwrap() } else { self.widget.take().unwrap() };
        let input = win.egui.take_egui_input(&win.window);
        let ctx = win.ctx.clone();
        let renderer = &mut win.renderer;
        let out = ctx.run_ui(input, |ui| {
            if is_main {
                self.draw_main(ui, renderer);
            } else {
                self.draw_widget(ui);
            }
        });
        win.egui.handle_platform_output(&win.window, out.platform_output);
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        win.repaint_at = if delay < Duration::from_secs(60) { Some(Instant::now() + delay) } else { None };
        let ppp = out.pixels_per_point;
        let prims = win.ctx.tessellate(out.shapes, ppp);
        for (id, deltas) in &out.textures_delta.set {
            for d in deltas {
                win.renderer.update_texture(&gpu.device, &gpu.queue, *id, d);
            }
        }
        let frame = match win.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            _ => {
                win.surface.configure(&gpu.device, &win.config);
                None
            }
        };
        if let Some(frame) = frame {
            let (x, y) = theme::TRAFFIC_LIGHTS;
            macos::inset_traffic_lights(&win.window, x, y);
            let view = frame.texture.create_view(&Default::default());
            let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: [win.config.width, win.config.height], pixels_per_point: ppp };
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            let extra = win.renderer.update_buffers(&gpu.device, &gpu.queue, &mut enc, &prims, &sd);
            {
                let bg = theme::palette(win.ctx.theme() == egui::Theme::Dark).window;
                let [r, g, b, _] = bg.to_normalized_gamma_f32();
                let mut pass = enc
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    })
                    .forget_lifetime();
                win.renderer.render(&mut pass, &prims, &sd);
            }
            gpu.queue.submit(extra.into_iter().chain([enc.finish()]));
            win.window.pre_present_notify();
            gpu.queue.present(frame);
        }
        for id in &out.textures_delta.free {
            win.renderer.free_texture(id);
        }
        if DRAG.swap(false, Ordering::Relaxed) {
            let _ = win.window.drag_window();
        }
        if is_main {
            self.main = Some(win);
        } else {
            self.widget = Some(win);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.menubar.is_some() {
            return;
        }
        let mtm = objc2::MainThreadMarker::new().expect("main thread");
        self.menubar = Some(menubar::MenuBar::new(mtm));
        let r = hotkey::register(hotkey::KEY_R, hotkey::CMD | hotkey::SHIFT | hotkey::OPTION, || push(Cmd::ToggleRecording));
        if let Err(code) = r {
            eprintln!("glide: could not register ⌥⇧⌘R (error {code})");
        }
        self.show_main(el);
        if let Some(p) = self.open_at_launch.take() {
            self.open_project(el, p);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, _: ()) {
        self.tick(el);
        for w in [self.main.as_ref(), self.widget.as_ref()].into_iter().flatten() {
            w.window.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let is_main = self.main.as_ref().is_some_and(|w| w.window.id() == id);
        let win = if is_main { self.main.as_mut() } else { self.widget.as_mut().filter(|w| w.window.id() == id) };
        let Some(win) = win else { return };
        let resp = win.egui.on_window_event(&win.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                if is_main {
                    if let View::Editor(ed) = &mut self.view {
                        ed.save();
                    }
                    push(Cmd::Quit);
                } else {
                    push(Cmd::Cancel);
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let s = win.window.inner_size();
                win.config.width = s.width.max(1);
                win.config.height = s.height.max(1);
                if let Some(g) = &self.gpu {
                    win.surface.configure(&g.device, &win.config);
                }
                win.window.request_redraw();
            }
            WindowEvent::ThemeChanged(_) => {
                theme::set_accent(theme::system_accent());
                theme::apply(&win.ctx);
                win.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.redraw(id);
            }
            WindowEvent::DroppedFile(path) => {
                if path.extension().and_then(|e| e.to_str()) == Some(crate::project::EXTENSION) {
                    push(Cmd::Open(path));
                }
            }
            _ => {
                if resp.repaint {
                    win.window.request_redraw();
                }
            }
        }
        let _ = el;
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.last_tick.elapsed() >= Duration::from_millis(100) {
            self.last_tick = Instant::now();
            self.tick(el);
        }
        let animating = match &mut self.view {
            View::Editor(ed) => ed.tick(),
            View::Home => false,
        };
        let live = self.recording.is_some() || self.finishing;
        let now = Instant::now();
        let mut next = now + Duration::from_millis(if live { 200 } else { 500 });
        if animating && let Some(w) = &self.main {
            w.window.request_redraw();
            next = now + Duration::from_millis(8);
        }
        if live && let Some(w) = &self.widget {
            w.window.request_redraw();
        }
        for w in [self.main.as_mut(), self.widget.as_mut()].into_iter().flatten() {
            if let Some(t) = w.repaint_at {
                if t <= now {
                    w.repaint_at = None;
                    w.window.request_redraw();
                } else {
                    next = next.min(t);
                }
            }
        }
        el.set_control_flow(ControlFlow::WaitUntil(next));
    }
}

/// Run the app (blocks until quit). `open` = a project to open at launch.
pub fn run(open: Option<PathBuf>) -> Result<()> {
    theme::set_accent(theme::system_accent());
    let el = EventLoop::new()?;
    let _ = PROXY.set(Mutex::new(el.create_proxy()));
    let mut app = App::new(open);
    el.run_app(&mut app)?;
    Ok(())
}
