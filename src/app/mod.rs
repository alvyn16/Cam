//! The Glide app: one main window (Home ↔ Editor), a floating recording
//! widget, a menu bar item and global shortcuts (default ⌥⇧⌘R). egui on wgpu with
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
pub mod voiceover;
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
    /// Show the area picker; `true` starts recording when confirmed.
    SelectArea(bool),
    /// Record a source chosen from the menu bar.
    RecordSource(SourceKind),
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
    /// Clear to transparent and skip titlebar chrome (area picker).
    overlay: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Main,
    Widget,
    Overlay,
}

/// Area picker state, in overlay-window points (= display points).
#[derive(Default)]
struct AreaPick {
    rect: Option<egui::Rect>,
    drag: Option<AreaDrag>,
    record_after: bool,
    display: u32,
    /// Set when the picker is confirmed or cancelled; the window closes.
    closed: bool,
}

#[derive(Clone, Copy)]
enum AreaDrag {
    New(egui::Pos2),
    Move(egui::Vec2),
    Corner(egui::Pos2),
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
    overlay: Option<Win>,
    area: AreaPick,
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
            overlay: None,
            area: AreaPick::default(),
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
        self.make_window(el, attrs, false)
    }

    /// Borderless transparent window covering a display (area picker).
    fn open_overlay(&mut self, el: &ActiveEventLoop, display: u32) -> Win {
        let (_, x, y, w, h) = macos::display_frame(display);
        let attrs = Window::default_attributes()
            .with_title("Select Area")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_position(winit::dpi::LogicalPosition::new(x, y))
            .with_inner_size(winit::dpi::LogicalSize::new(w, h))
            .with_window_level(WindowLevel::AlwaysOnTop);
        self.make_window(el, attrs, true)
    }

    fn make_window(&mut self, el: &ActiveEventLoop, attrs: winit::window::WindowAttributes, overlay: bool) -> Win {
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
            alpha_mode: if overlay {
                [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied]
                    .into_iter()
                    .find(|m| caps.alpha_modes.contains(m))
                    .unwrap_or(caps.alpha_modes[0])
            } else {
                caps.alpha_modes[0]
            },
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
        if !overlay {
            let (x, y) = theme::TRAFFIC_LIGHTS;
            macos::inset_traffic_lights(&window, x, y);
        }
        window.request_redraw();
        Win { window, surface, config, egui, ctx, renderer, repaint_at: None, overlay }
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

    /// (Re)register the global shortcuts from settings.
    fn bind_hotkeys(&mut self) {
        let hk = self.settings.hotkeys.clone();
        let mut failed = vec![];
        for (slot, combo, cmd) in [(1, hk.record, Cmd::ToggleRecording), (2, hk.pause, Cmd::TogglePause), (3, hk.cancel, Cmd::Cancel)] {
            match combo {
                Some(c) => {
                    if let Err(code) = hotkey::bind(slot, c, move || push(cmd.clone())) {
                        eprintln!("glide: could not register {} (error {code})", c.label());
                        failed.push(c.label());
                    }
                }
                None => hotkey::unbind(slot),
            }
        }
        if let Some(m) = &self.menubar {
            m.set_hotkey(&hk.record_label(), !failed.iter().any(|l| *l == hk.record_label()));
        }
        if !failed.is_empty() {
            self.error = Some(format!("Another app already uses {} — pick a different shortcut.", failed.join(", ")));
        }
    }

    fn recorder_options(&self) -> recorder::Options {
        let s = &self.settings;
        if self.selftest.is_some() {
            return recorder::Options { target: Target::Pattern, system_audio: false, mic: None, camera: None, fps: 60, keys: false, dir: s.dir(), name: Some("Selftest".into()), ..Default::default() };
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
            SourceKind::Device => match &s.ios_device {
                Some(id) => Target::Device { id: id.clone() },
                None => Target::Display { id: s.display },
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
            hide_desktop_icons: s.hide_desktop_icons,
            hide_dock: s.hide_dock,
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
            Cmd::RecordSource(kind) => {
                if self.recording.is_some() {
                    return;
                }
                self.settings.source = kind;
                match kind {
                    SourceKind::Area => push(Cmd::SelectArea(true)),
                    // Pick which window in the main window first.
                    SourceKind::Window => {
                        if let View::Editor(ed) = &mut self.view {
                            ed.save();
                        }
                        self.view = View::Home;
                        self.devices = Devices::load();
                        self.show_main(el);
                    }
                    _ => push(Cmd::ToggleRecording),
                }
            }
            Cmd::SelectArea(record_after) => {
                if self.recording.is_some() {
                    return;
                }
                let display = self.settings.display;
                let (id, ..) = macos::display_frame(display);
                let s = &self.settings;
                // Start from the saved area when it was on this display.
                let rect = (s.source == SourceKind::Area && s.area[2] > 8.0 && s.area[3] > 8.0)
                    .then(|| egui::Rect::from_min_size(egui::pos2(s.area[0] as f32, s.area[1] as f32), vec2(s.area[2] as f32, s.area[3] as f32)));
                self.area = AreaPick { rect, drag: None, record_after, display: id, closed: false };
                if self.overlay.is_none() {
                    let w = self.open_overlay(el, display);
                    self.overlay = Some(w);
                }
                if let Some(w) = &self.overlay {
                    w.window.focus_window();
                    w.window.request_redraw();
                }
                macos::activate();
            }
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
                    if let View::Editor(ed) = &mut self.view {
                        ed.fresh = true;
                    }
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
                st.phase = 10;
                st.at = Instant::now();
                push(Cmd::SelectArea(false));
            }
            10 if since > Duration::from_millis(600) => {
                let Some(w) = &self.overlay else {
                    fail("area picker did not open".into());
                    return;
                };
                let size = w.window.inner_size();
                log(&format!("area picker open {}x{} px", size.width, size.height));
                if let (Some(dir), Some(num)) = (std::env::var_os("GLIDE_SELFTEST_SHOTS"), macos::window_number(&w.window)) {
                    let out = PathBuf::from(dir).join("area-picker.png");
                    let ok = std::process::Command::new("screencapture").args(["-x", "-o", "-l", &num.to_string()]).arg(&out).status().is_ok_and(|s| s.success());
                    log(&format!("area picker screenshot {} ok={ok}", out.display()));
                }
                st.phase = 11;
                st.at = Instant::now();
                self.finish_area(Some(egui::Rect::from_min_size(egui::pos2(200.0, 150.0), vec2(640.0, 400.0))));
            }
            11 if since > Duration::from_millis(300) => {
                let a = self.settings.area;
                log(&format!("area saved source={:?} area={:?} overlay_closed={}", self.settings.source, a, self.overlay.is_none()));
                if self.settings.source != SourceKind::Area || a != [200.0, 150.0, 640.0, 400.0] || self.overlay.is_some() {
                    fail("area picker result".into());
                }
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
                let was = home::CAPTURING_SHORTCUT.swap(false, Ordering::Relaxed);
                let note = self.permission_note.clone();
                let action = home::ui(ui, &mut self.settings, &mut self.devices, &mut self.projects, &mut self.thumbs, note.as_deref());
                match action {
                    // Area recordings confirm the region first.
                    home::Action::Record if self.settings.source == SourceKind::Area => push(Cmd::SelectArea(true)),
                    home::Action::Record => push(Cmd::ToggleRecording),
                    home::Action::SelectArea => push(Cmd::SelectArea(false)),
                    home::Action::HotkeysChanged => {
                        self.settings.save();
                        self.bind_hotkeys();
                    }
                    home::Action::Open(p) => push(Cmd::Open(p)),
                    home::Action::RequestPermissions => {
                        std::thread::spawn(|| crate::capture::tcc::request_all(true, false, false, Duration::from_secs(60)));
                    }
                    home::Action::None => {}
                }
                let now = home::CAPTURING_SHORTCUT.load(Ordering::Relaxed);
                if now && !was {
                    for slot in 1..=3 {
                        hotkey::unbind(slot);
                    }
                } else if was && !now {
                    self.bind_hotkeys();
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
                            if widgets::record_button(ui, 26.0, true).on_hover_text(format!("Stop {}", self.settings.hotkeys.record_label())).clicked() {
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

    /// Area picker: dim the display, drag a rect, confirm with Return.
    fn draw_overlay(&mut self, ui: &mut egui::Ui) {
        use egui::{Align2, FontId, Pos2, Rect, Stroke, pos2};
        let full = ui.max_rect();
        let resp = ui.interact(full, ui.id().with("area"), Sense::click_and_drag());
        let pointer = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()));
        let a = &mut self.area;
        let corners = |r: Rect| [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()];
        if resp.drag_started()
            && let Some(p) = pointer
        {
            a.drag = Some(match a.rect {
                Some(r) => match corners(r).into_iter().find(|c| c.distance(p) < 12.0) {
                    // Resize from the opposite (anchored) corner.
                    Some(c) => AreaDrag::Corner(pos2(r.left() + r.right() - c.x, r.top() + r.bottom() - c.y)),
                    None if r.contains(p) => AreaDrag::Move(p - r.min),
                    None => AreaDrag::New(p),
                },
                None => AreaDrag::New(p),
            });
        }
        if resp.dragged()
            && let (Some(d), Some(p)) = (a.drag, pointer)
        {
            let p = p.clamp(full.min, full.max);
            a.rect = Some(match d {
                AreaDrag::New(start) | AreaDrag::Corner(start) => Rect::from_two_pos(start, p),
                AreaDrag::Move(off) => {
                    let r = a.rect.unwrap_or(Rect::NOTHING);
                    let min = (p - off).clamp(full.min, full.max - r.size());
                    Rect::from_min_size(min, r.size())
                }
            });
        }
        if resp.drag_stopped() {
            a.drag = None;
            if a.rect.is_some_and(|r| r.width() < 16.0 || r.height() < 16.0) {
                a.rect = None;
            }
        }

        // Dim everything outside the selection.
        let painter = ui.painter();
        let dim = Color32::from_black_alpha(120);
        match a.rect {
            Some(r) => {
                painter.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, r.top())), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(full.min.x, r.bottom()), full.max), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(full.min.x, r.top()), pos2(r.left(), r.bottom())), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(r.right(), r.top()), pos2(full.max.x, r.bottom())), 0.0, dim);
                painter.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
                for c in corners(r) {
                    painter.circle(c, 5.0, Color32::WHITE, Stroke::new(1.0, theme::accent()));
                }
                let label = format!("{:.0} × {:.0}", r.width(), r.height());
                let at = if r.top() > 30.0 { r.center_top() - vec2(0.0, 10.0) } else { r.center_top() + vec2(0.0, 18.0) };
                painter.text(at, Align2::CENTER_BOTTOM, label, FontId::proportional(13.0), Color32::WHITE);
                let hover = pointer.map(|p| (corners(r).iter().any(|c| c.distance(p) < 12.0), r.contains(p)));
                ui.ctx().set_cursor_icon(match hover {
                    Some((true, _)) => egui::CursorIcon::ResizeNwSe,
                    Some((false, true)) => egui::CursorIcon::Move,
                    _ => egui::CursorIcon::Crosshair,
                });
            }
            None => {
                painter.rect_filled(full, 0.0, dim);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }
        let hint = "Drag to select an area  ·  Return to confirm  ·  Esc to cancel";
        let hint_pos = pos2(full.center().x, full.top() + 60.0);
        let galley = painter.layout_no_wrap(hint.into(), FontId::proportional(14.0), Color32::WHITE);
        let bg = Rect::from_center_size(hint_pos, galley.size() + vec2(28.0, 14.0));
        painter.rect_filled(bg, egui::CornerRadius::same(10), Color32::from_black_alpha(170));
        painter.galley(bg.center() - galley.size() / 2.0, galley, Color32::WHITE);

        // Buttons under the selection.
        let mut confirm = ui.input(|i| i.key_pressed(egui::Key::Enter)) && a.rect.is_some();
        let mut cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if let Some(r) = a.rect
            && a.drag.is_none()
        {
            let y = if r.bottom() + 52.0 < full.bottom() { r.bottom() + 12.0 } else { r.bottom() - 44.0 };
            let bar = Rect::from_min_size(Pos2::new(r.right() - 220.0, y), vec2(220.0, 32.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
                let primary = if a.record_after { "Record" } else { "Use area" };
                if widgets::primary(ui, primary).clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        }
        let outcome = if confirm { a.rect.map(|r| r.translate(-full.min.to_vec2())) } else { None };
        if confirm || cancel {
            self.finish_area(outcome);
        }
    }

    /// Close the area picker; with a rect, save it (and record if asked).
    fn finish_area(&mut self, rect: Option<egui::Rect>) {
        self.area.closed = true;
        self.overlay = None;
        match rect {
            Some(r) => {
                self.settings.source = SourceKind::Area;
                self.settings.display = self.area.display;
                self.settings.area = [r.left() as f64, r.top() as f64, r.width() as f64, r.height() as f64];
                if self.selftest.is_none() {
                    self.settings.save();
                }
                push(if self.area.record_after { Cmd::ToggleRecording } else { Cmd::ShowWindow });
            }
            None => push(Cmd::ShowWindow),
        }
    }

    fn which(&self, id: WindowId) -> Option<Which> {
        let is = |w: &Option<Win>| w.as_ref().is_some_and(|w| w.window.id() == id);
        if is(&self.main) {
            Some(Which::Main)
        } else if is(&self.widget) {
            Some(Which::Widget)
        } else if is(&self.overlay) {
            Some(Which::Overlay)
        } else {
            None
        }
    }

    fn slot(&mut self, which: Which) -> &mut Option<Win> {
        match which {
            Which::Main => &mut self.main,
            Which::Widget => &mut self.widget,
            Which::Overlay => &mut self.overlay,
        }
    }

    fn redraw(&mut self, id: WindowId) {
        let Some(which) = self.which(id) else { return };
        let Some(gpu) = self.gpu.clone() else { return };
        let Some(mut win) = self.slot(which).take() else { return };
        let input = win.egui.take_egui_input(&win.window);
        let ctx = win.ctx.clone();
        let renderer = &mut win.renderer;
        // The overlay may close itself while drawing; remember that.
        let mut keep = true;
        let out = ctx.run_ui(input, |ui| match which {
            Which::Main => self.draw_main(ui, renderer),
            Which::Widget => self.draw_widget(ui),
            Which::Overlay => {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    self.draw_overlay(ui);
                });
                keep = !self.area.closed;
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
            if !win.overlay {
                let (x, y) = theme::TRAFFIC_LIGHTS;
                macos::inset_traffic_lights(&win.window, x, y);
            }
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
                                load: wgpu::LoadOp::Clear(if win.overlay {
                                    wgpu::Color::TRANSPARENT
                                } else {
                                    wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }
                                }),
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
        if keep {
            *self.slot(which) = Some(win);
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
        self.bind_hotkeys();
        self.show_main(el);
        if let Some(p) = self.open_at_launch.take() {
            self.open_project(el, p);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, _: ()) {
        self.tick(el);
        for w in [self.main.as_ref(), self.widget.as_ref(), self.overlay.as_ref()].into_iter().flatten() {
            w.window.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(which) = self.which(id) else { return };
        if matches!(event, WindowEvent::CloseRequested) {
            match which {
                Which::Main => {
                    if let View::Editor(ed) = &mut self.view {
                        ed.save();
                    }
                    push(Cmd::Quit);
                }
                Which::Widget => push(Cmd::Cancel),
                Which::Overlay => {
                    self.overlay = None;
                    push(Cmd::ShowWindow);
                }
            }
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.redraw(id);
            return;
        }
        let gpu = self.gpu.clone();
        let Some(win) = self.slot(which).as_mut() else { return };
        let resp = win.egui.on_window_event(&win.window, &event);
        match event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let s = win.window.inner_size();
                win.config.width = s.width.max(1);
                win.config.height = s.height.max(1);
                if let Some(g) = &gpu {
                    win.surface.configure(&g.device, &win.config);
                }
                win.window.request_redraw();
            }
            WindowEvent::ThemeChanged(_) => {
                theme::set_accent(theme::system_accent());
                theme::apply(&win.ctx);
                win.window.request_redraw();
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
        for w in [self.main.as_mut(), self.widget.as_mut(), self.overlay.as_mut()].into_iter().flatten() {
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
    // Let connected iPhones/iPads appear as recordable devices.
    crate::capture::camera::enable_screen_capture_devices();
    let el = EventLoop::new()?;
    let _ = PROXY.set(Mutex::new(el.create_proxy()));
    macos::install_open_handler();
    let mut app = App::new(open);
    el.run_app(&mut app)?;
    Ok(())
}
