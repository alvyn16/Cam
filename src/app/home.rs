//! Recording setup and recent recordings: the window's start state (the
//! editor layout with no recording open) and the editor's title-bar controls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use egui::{Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Ui, vec2};
use serde::{Deserialize, Serialize};

use super::hotkey::Combo;
use super::theme::{self, pal};
use super::widgets::{self, caption, row, segmented, toggle_row};
use crate::project::{self, Package};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    #[default]
    Display,
    Window,
    Area,
    /// A USB-connected iPhone or iPad.
    Device,
}

/// Recording preferences, remembered between launches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source: SourceKind,
    pub display: u32,
    pub window: Option<u32>,
    /// Area as x, y, w, h in display points.
    pub area: [f64; 4],
    pub system_audio: bool,
    pub mic: bool,
    pub mic_id: Option<String>,
    pub camera: bool,
    pub camera_id: Option<String>,
    pub keys: bool,
    pub countdown: bool,
    pub fps: u32,
    pub projects_dir: Option<PathBuf>,
    pub hide_desktop_icons: bool,
    pub hide_dock: bool,
    pub hotkeys: Hotkeys,
    /// iPhone/iPad unique id.
    pub ios_device: Option<String>,
    /// Window sizes (points) saved from the Resize menu.
    pub window_sizes: Vec<(u32, u32)>,
}

/// Global shortcuts (None = unbound).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub record: Option<Combo>,
    pub pause: Option<Combo>,
    pub cancel: Option<Combo>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        use super::hotkey::{CMD, KEY_R, OPTION, SHIFT};
        Hotkeys {
            record: Some(Combo::new(KEY_R, CMD | SHIFT | OPTION)),
            pause: Some(Combo::new(35, CMD | SHIFT | OPTION)), // P
            cancel: None,
        }
    }
}

impl Hotkeys {
    pub fn record_label(&self) -> String {
        self.record.map_or_else(String::new, |c| c.label())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            source: SourceKind::Display,
            display: 0,
            window: None,
            area: [200.0, 150.0, 1280.0, 720.0],
            system_audio: true,
            mic: false,
            mic_id: None,
            camera: false,
            camera_id: None,
            keys: true,
            countdown: true,
            fps: 60,
            projects_dir: None,
            hide_desktop_icons: false,
            hide_dock: false,
            hotkeys: Hotkeys::default(),
            ios_device: None,
            window_sizes: vec![],
        }
    }
}

fn settings_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/Glide/settings.json")
}

impl Settings {
    pub fn load() -> Self {
        project::read_json(&settings_path()).unwrap_or_default()
    }
    pub fn save(&self) {
        let p = settings_path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = project::write_json(&p, self);
    }
    pub fn dir(&self) -> PathBuf {
        self.projects_dir.clone().unwrap_or_else(project::projects_dir)
    }
}

#[derive(Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// A window that can be recorded.
#[derive(Clone, Debug)]
pub struct WindowEntry {
    pub id: u32,
    pub label: String,
    /// Size in points.
    pub size: (f64, f64),
    /// Size of the display it's on, in points.
    pub display: (f64, f64),
}

#[derive(Default)]
pub struct Devices {
    pub displays: Vec<(u32, String)>,
    pub windows: Vec<WindowEntry>,
    pub cameras: Vec<Device>,
    pub mics: Vec<Device>,
    pub ios: Vec<Device>,
}

impl Devices {
    pub fn load() -> Self {
        use crate::capture::{camera, screen};
        Devices {
            displays: screen::displays()
                .map(|ds| ds.iter().enumerate().map(|(i, d)| (d.id, format!("Display {} — {}×{}", i + 1, d.points.0, d.points.1))).collect())
                .unwrap_or_default(),
            windows: screen::windows()
                .map(|ws| {
                    let ws: Vec<_> = ws.into_iter().filter(|w| !w.title.is_empty() && w.app != "Glide").collect();
                    let displays = crate::capture::winsize::display_sizes(&ws);
                    ws.into_iter()
                        .zip(displays)
                        .map(|(w, display)| WindowEntry { id: w.id, label: format!("{} — {}", w.app, w.title), size: w.size, display })
                        .collect()
                })
                .unwrap_or_default(),
            cameras: camera::cameras().into_iter().map(|d| Device { id: d.id, name: d.name }).collect(),
            mics: camera::microphones().into_iter().map(|d| Device { id: d.id, name: d.name }).collect(),
            ios: camera::ios_devices().into_iter().map(|d| Device { id: d.id, name: d.name }).collect(),
        }
    }
}

pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    pub modified: SystemTime,
    pub duration_ms: f64,
}

pub fn scan_projects(dir: &Path) -> Vec<ProjectEntry> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some(project::EXTENSION) {
                continue;
            }
            let pkg = Package::new(&path);
            let Ok(meta) = project::read_json::<crate::project::recording::RecordingMeta>(&pkg.rec("meta.json")) else { continue };
            let modified = std::fs::metadata(pkg.project_file()).or_else(|_| e.metadata()).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(ProjectEntry { name: pkg.name(), path, modified, duration_ms: meta.duration_ms });
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// Thumbnails rendered on a background thread and cached on disk.
pub struct Thumbs {
    pub textures: HashMap<PathBuf, egui::TextureHandle>,
    pending: Arc<Mutex<Vec<(PathBuf, crate::render::Image)>>>,
    requested: std::collections::HashSet<PathBuf>,
    /// One background worker renders thumbnails in order (each needs a
    /// decoder and a GPU renderer, so they're never made in parallel).
    queue: Option<std::sync::mpsc::Sender<(PathBuf, egui::Context)>>,
}

impl Default for Thumbs {
    fn default() -> Self {
        Thumbs { textures: HashMap::new(), pending: Arc::new(Mutex::new(vec![])), requested: Default::default(), queue: None }
    }
}

impl Thumbs {
    pub fn get(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureId> {
        for (p, img) in self.pending.lock().unwrap().drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba);
            self.textures.insert(p.clone(), ctx.load_texture(p.to_string_lossy(), ci, egui::TextureOptions::LINEAR));
        }
        if let Some(t) = self.textures.get(path) {
            return Some(t.id());
        }
        if self.requested.insert(path.to_path_buf()) {
            let pending = self.pending.clone();
            let tx = self.queue.get_or_insert_with(|| {
                let (tx, rx) = std::sync::mpsc::channel::<(PathBuf, egui::Context)>();
                std::thread::spawn(move || {
                    for (p, ctx) in rx {
                        if let Some(img) = thumbnail(&p) {
                            pending.lock().unwrap().push((p, img));
                            ctx.request_repaint();
                        }
                    }
                });
                tx
            });
            let _ = tx.send((path.to_path_buf(), ctx.clone()));
        }
        None
    }

    pub fn forget(&mut self, path: &Path) {
        self.textures.remove(path);
        self.requested.remove(path);
    }
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Load `thumb.png` (unless the project changed since) or render one from a
/// moment without a zoom, so it shows the whole composition.
fn thumbnail(path: &Path) -> Option<crate::render::Image> {
    let pkg = Package::new(path);
    let thumb = path.join("thumb.png");
    if modified(&thumb) >= modified(&pkg.project_file())
        && let Ok(img) = image::open(&thumb)
    {
        let img = img.to_rgba8();
        return Some(crate::render::Image { width: img.width(), height: img.height(), rgba: img.into_raw() });
    }
    objc2::rc::autoreleasepool(|_| {
        let (p, rec) = pkg.load().ok()?;
        let mut scene = crate::engine::scene::Scene::new(p.clone(), &rec, 30.0);
        let d = scene.duration();
        let t = [(d * 0.25).min(1500.0), d * 0.1, d * 0.5, d * 0.75, 0.0]
            .into_iter()
            .find(|&t| crate::engine::zoom::active(&p.timeline.zooms, scene.map.to_source(t)).is_none())
            .unwrap_or(0.0);
        let frame = scene.frame(t, 270.0);
        let gpu = crate::render::Gpu::headless().ok()?;
        let mut r = crate::render::Renderer::new(gpu);
        r.prepare(&p.style, &rec, Some(path));
        let mut src = crate::media::FrameSource::open(&pkg.rec(&rec.meta.screen.file)).ok()?;
        if let Some(f) = src.frame_at(frame.source_ms) {
            f.with_bgra(|v| r.set_screen(v));
        }
        if let Some(cam) = &rec.meta.camera
            && let Ok(mut c) = crate::media::FrameSource::open(&pkg.rec(&cam.file))
            && let Some(f) = c.frame_at(frame.source_ms - cam.offset_ms)
        {
            f.with_bgra(|v| r.set_camera(v));
        }
        let img = r.render_to_image(&frame, &p.style);
        let _ = image::save_buffer(&thumb, &img.rgba, img.width, img.height, image::ExtendedColorType::Rgba8);
        Some(img)
    })
}

/// True while a shortcut button waits for keys (global hotkeys are paused
/// so the old combination reaches the field instead of starting a recording).
pub static CAPTURING_SHORTCUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A shortcut button that records the next key combination when clicked.
fn shortcut_row(ui: &mut Ui, label: &str, combo: &mut Option<Combo>) -> bool {
    let id = ui.id().with(("shortcut", label));
    let capturing = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if capturing {
        CAPTURING_SHORTCUT.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let mut changed = false;
    row(ui, label, |ui| {
        let text = if capturing { "Press keys…".to_string() } else { combo.map_or("None".into(), |c| c.label()) };
        let b = ui.add(egui::Button::new(text).selected(capturing).min_size(vec2(110.0, 0.0)));
        if b.clicked() {
            ui.data_mut(|d| {
                d.insert_temp(id, !capturing);
            });
        }
        if capturing {
            let pressed = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            match pressed {
                Some((egui::Key::Escape, _)) => {
                    ui.data_mut(|d| d.insert_temp(id, false));
                }
                Some((egui::Key::Backspace | egui::Key::Delete, _)) => {
                    *combo = None;
                    changed = true;
                    ui.data_mut(|d| d.insert_temp(id, false));
                }
                Some((k, m)) => {
                    if let Some(c) = Combo::from_egui(k, m) {
                        *combo = Some(c);
                        changed = true;
                        ui.data_mut(|d| d.insert_temp(id, false));
                    }
                }
                None => {}
            }
        }
    });
    changed
}

pub enum Action {
    None,
    Record,
    SelectArea,
    HotkeysChanged,
    Open(PathBuf),
    RequestPermissions,
    /// Close the open recording (back to recording setup).
    Back,
    ResizeWindow { id: u32, width: u32, height: u32 },
    /// Pick a window on screen.
    SelectWindow,
}

/// Outcome of the last window resize, shown under the window picker.
static RESIZE_NOTE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn set_resize_note(note: Option<String>) {
    *RESIZE_NOTE.lock().unwrap_or_else(|e| e.into_inner()) = note;
}

fn size_label((w, h): (u32, u32)) -> String {
    format!("{w} × {h}")
}

/// The chosen window's size with a Resize menu (common sizes, saved sizes,
/// custom). Returns the size picked this frame.
pub fn window_size_row(ui: &mut Ui, s: &mut Settings, w: &WindowEntry) -> Option<(u32, u32)> {
    use crate::capture::winsize::PRESETS;
    let current = (w.size.0.round() as u32, w.size.1.round() as u32);
    let fits = |(a, b): (u32, u32)| a as f64 <= w.display.0 && b as f64 <= w.display.1 && (a, b) != current;
    let custom_id = ui.id().with("custom-window-size");
    let mut custom: Option<[u32; 2]> = ui.data(|d| d.get_temp(custom_id));
    let mut picked = None;
    ui.horizontal(|ui| {
        caption(ui, &format!("{} pt", size_label(current)));
        ui.menu_button("Resize", |ui| {
            for &size in PRESETS[0].1 {
                if ui.add_enabled(fits(size), egui::Button::new(size_label(size))).clicked() {
                    picked = Some(size);
                }
            }
            for (name, sizes) in &PRESETS[1..] {
                ui.menu_button(*name, |ui| {
                    for &size in *sizes {
                        if ui.add_enabled(fits(size), egui::Button::new(size_label(size))).clicked() {
                            picked = Some(size);
                        }
                    }
                });
            }
            ui.separator();
            if s.window_sizes.is_empty() {
                ui.add_enabled(false, egui::Button::new("No saved sizes"));
            }
            for &size in &s.window_sizes {
                if ui.add_enabled(fits(size), egui::Button::new(size_label(size))).clicked() {
                    picked = Some(size);
                }
            }
            let known = PRESETS.iter().any(|(_, l)| l.contains(&current)) || s.window_sizes.contains(&current);
            if ui.add_enabled(!known, egui::Button::new("Save current size")).clicked() {
                s.window_sizes.push(current);
            }
            if !s.window_sizes.is_empty() {
                ui.menu_button("Delete saved size", |ui| {
                    let mut gone = None;
                    for &size in &s.window_sizes {
                        if ui.button(size_label(size)).clicked() {
                            gone = Some(size);
                        }
                    }
                    if let Some(g) = gone {
                        s.window_sizes.retain(|x| *x != g);
                    }
                });
            }
            ui.separator();
            if ui.button("Custom…").clicked() {
                custom = Some([current.0, current.1]);
            }
        });
    });
    if let Some(c) = &mut custom {
        let mut done = false;
        ui.horizontal(|ui| {
            let max = (w.display.0.min(10_000.0) as u32, w.display.1.min(10_000.0) as u32);
            ui.add(egui::DragValue::new(&mut c[0]).range(200..=max.0).suffix(" w"));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut c[1]).range(200..=max.1).suffix(" h"));
            // Consume Return/Esc so a surrounding picker doesn't act on them too.
            if ui.button("Resize").clicked() || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                picked = Some((c[0], c[1]));
                done = true;
            }
            if ui.button("Cancel").clicked() || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                done = true;
            }
        });
        if done {
            custom = None;
        }
    }
    ui.data_mut(|d| match custom {
        Some(c) => {
            d.insert_temp(custom_id, c);
        }
        None => {
            d.remove::<[u32; 2]>(custom_id);
        }
    });
    if let Some(note) = RESIZE_NOTE.lock().unwrap_or_else(|e| e.into_inner()).as_deref() {
        caption(ui, note);
    }
    picked
}

/// Recording setup for the inspector: source, audio, camera and options.
pub fn record_settings(ui: &mut Ui, s: &mut Settings, dev: &mut Devices, permission_note: Option<&str>) -> Action {
    let mut action = Action::None;
    ui.label(RichText::new("Source").font(theme::semibold(13.0)).color(pal(ui).secondary));
    ui.add_space(4.0);
    segmented(ui, &mut s.source, &[(SourceKind::Display, "Display"), (SourceKind::Window, "Window"), (SourceKind::Area, "Area"), (SourceKind::Device, "iPhone")]);
    ui.add_space(8.0);
    match s.source {
        SourceKind::Display => {
            let label = dev.displays.iter().find(|d| d.0 == s.display).map_or("Main display".to_string(), |d| d.1.clone());
            egui::ComboBox::from_id_salt("display").width(290.0).selected_text(label).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.display, 0, "Main display");
                for (id, name) in &dev.displays {
                    ui.selectable_value(&mut s.display, *id, name);
                }
            });
        }
        SourceKind::Window => {
            let chosen = s.window.and_then(|w| dev.windows.iter().find(|d| d.id == w)).cloned();
            let label = chosen.as_ref().map_or("Choose a window".into(), |d| d.label.clone());
            egui::ComboBox::from_id_salt("window").width(290.0).selected_text(label).show_ui(ui, |ui| {
                for d in &dev.windows {
                    ui.selectable_value(&mut s.window, Some(d.id), &d.label);
                }
            });
            if let Some(w) = &chosen
                && let Some((width, height)) = window_size_row(ui, s, w)
            {
                action = Action::ResizeWindow { id: w.id, width, height };
            }
            ui.horizontal(|ui| {
                if ui.small_button("Pick on screen…").on_hover_text("Highlight windows on the display and click one").clicked() {
                    action = Action::SelectWindow;
                }
                if ui.small_button("Refresh windows").clicked() {
                    *dev = Devices::load();
                }
            });
        }
        SourceKind::Device => {
            if dev.ios.is_empty() {
                caption(ui, "Connect an iPhone or iPad with a cable, unlock it and trust this Mac.");
            } else {
                if s.ios_device.as_ref().is_none_or(|id| !dev.ios.iter().any(|d| &d.id == id)) {
                    s.ios_device = dev.ios.first().map(|d| d.id.clone());
                }
                let label = s.ios_device.as_ref().and_then(|id| dev.ios.iter().find(|d| &d.id == id)).map_or("", |d| d.name.as_str()).to_string();
                egui::ComboBox::from_id_salt("ios").width(290.0).selected_text(label).show_ui(ui, |ui| {
                    for d in &dev.ios {
                        ui.selectable_value(&mut s.ios_device, Some(d.id.clone()), &d.name);
                    }
                });
            }
            if ui.small_button("Refresh").clicked() {
                *dev = Devices::load();
            }
        }
        SourceKind::Area => {
            ui.horizontal(|ui| {
                if ui.button("Select area…").on_hover_text("Drag a region on the screen").clicked() {
                    action = Action::SelectArea;
                }
                caption(ui, &format!("{:.0} × {:.0} at {:.0}, {:.0}", s.area[2], s.area[3], s.area[0], s.area[1]));
            });
            egui::CollapsingHeader::new("Exact size").show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, l) in ["x", "y", "w", "h"].iter().enumerate() {
                        ui.label(*l);
                        ui.add(egui::DragValue::new(&mut s.area[i]).speed(4.0).range(0.0..=8000.0));
                    }
                });
                ui.horizontal(|ui| {
                    for (name, w, h) in [("1280×720", 1280.0, 720.0), ("1920×1080", 1920.0, 1080.0), ("1080×1080", 1080.0, 1080.0)] {
                        if ui.small_button(name).clicked() {
                            s.area[2] = w;
                            s.area[3] = h;
                        }
                    }
                });
            });
        }
    }
    ui.add_space(10.0);
    ui.separator();
    toggle_row(ui, "System audio", &mut s.system_audio);
    toggle_row(ui, "Microphone", &mut s.mic);
    if s.mic {
        let label = s.mic_id.as_ref().and_then(|id| dev.mics.iter().find(|m| &m.id == id)).map_or("Default".to_string(), |m| m.name.clone());
        egui::ComboBox::from_id_salt("mic").width(290.0).selected_text(label).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.mic_id, None, "Default");
            for m in &dev.mics {
                ui.selectable_value(&mut s.mic_id, Some(m.id.clone()), &m.name);
            }
        });
        if crate::capture::camera::builtin_mic_off(s.mic_id.as_deref()) {
            caption(ui, "The lid is closed, so the built-in microphone only records silence. Choose another microphone.");
        }
    }
    toggle_row(ui, "Camera", &mut s.camera);
    if s.camera {
        let label = s.camera_id.as_ref().and_then(|id| dev.cameras.iter().find(|m| &m.id == id)).map_or("Default".to_string(), |m| m.name.clone());
        egui::ComboBox::from_id_salt("cam").width(290.0).selected_text(label).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.camera_id, None, "Default");
            for m in &dev.cameras {
                ui.selectable_value(&mut s.camera_id, Some(m.id.clone()), &m.name);
            }
        });
    }
    toggle_row(ui, "Record key presses", &mut s.keys);
    toggle_row(ui, "3 s countdown", &mut s.countdown);
    if matches!(s.source, SourceKind::Display | SourceKind::Area) {
        toggle_row(ui, "Hide desktop icons", &mut s.hide_desktop_icons);
        toggle_row(ui, "Hide Dock", &mut s.hide_dock);
    }
    row(ui, "Frame rate", |ui| {
        segmented(ui, &mut s.fps, &[(60, "60"), (30, "30")]);
    });
    egui::CollapsingHeader::new("Global shortcuts").show(ui, |ui| {
        let mut changed = false;
        changed |= shortcut_row(ui, "Start / stop", &mut s.hotkeys.record);
        changed |= shortcut_row(ui, "Pause / resume", &mut s.hotkeys.pause);
        changed |= shortcut_row(ui, "Cancel recording", &mut s.hotkeys.cancel);
        caption(ui, "Click a shortcut, then press the new keys (⌫ clears, ⎋ keeps it).");
        if changed {
            action = Action::HotkeysChanged;
        }
    });
    if let Some(note) = permission_note {
        ui.add_space(12.0);
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(255, 159, 10, 30))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(RichText::new(note).size(12.0));
                if ui.button("Request access").clicked() {
                    action = Action::RequestPermissions;
                }
            });
    }
    action
}

/// One line describing what Record will capture.
pub fn source_summary(s: &Settings, dev: &Devices) -> String {
    let source = match s.source {
        SourceKind::Display => dev.displays.iter().find(|d| d.0 == s.display).map_or("Main display".to_string(), |d| d.1.split(" — ").next().unwrap_or("Display").to_string()),
        SourceKind::Window => s.window.and_then(|w| dev.windows.iter().find(|d| d.id == w)).map_or("A window (pick it next)".to_string(), |d| d.label.clone()),
        SourceKind::Area => format!("Area {:.0} × {:.0}", s.area[2], s.area[3]),
        SourceKind::Device => s.ios_device.as_ref().and_then(|id| dev.ios.iter().find(|d| &d.id == id)).map_or("iPhone / iPad".to_string(), |d| d.name.clone()),
    };
    let on = |b: bool| if b { "on" } else { "off" };
    format!("{source}  ·  system audio {}  ·  mic {}  ·  camera {}", on(s.system_audio), on(s.mic), on(s.camera))
}

fn source_name(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Display => "Display",
        SourceKind::Window => "Window",
        SourceKind::Area => "Area",
        SourceKind::Device => "iPhone",
    }
}

/// Recent recordings, newest first, with small thumbnails; plus Open… and
/// the projects folder. Used as the popover off the window title.
pub fn recents(ui: &mut Ui, projects: &[ProjectEntry], thumbs: &mut Thumbs, current: Option<&Path>, dir: &Path) -> Action {
    let mut action = Action::None;
    let p = pal(ui);
    const WIDTH: f32 = 340.0;
    ui.set_width(WIDTH);
    ui.label(RichText::new("Recent recordings").font(theme::semibold(12.0)).color(p.secondary));
    if projects.is_empty() {
        caption(ui, "Nothing recorded yet.");
    }
    for e in projects.iter().take(10) {
        let open = current == Some(e.path.as_path());
        let (rect, resp) = ui.allocate_exact_size(vec2(WIDTH, 46.0), Sense::click());
        if resp.hovered() || open {
            ui.painter().rect_filled(rect, CornerRadius::same(6), if open { p.pressed } else { p.hover });
        }
        let thumb = egui::Rect::from_min_size(rect.min + vec2(4.0, 5.0), vec2(58.0, 36.0));
        ui.painter().rect_filled(thumb, CornerRadius::same(4), p.control);
        if let Some(tex) = thumbs.get(ui.ctx(), &e.path) {
            ui.painter().image(tex, thumb, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        let x = thumb.right() + 10.0;
        ui.painter().text(egui::pos2(x, rect.top() + 7.0), egui::Align2::LEFT_TOP, &e.name, theme::semibold(12.5), p.text);
        ui.painter().text(
            egui::pos2(x, rect.top() + 25.0),
            egui::Align2::LEFT_TOP,
            format!("{}  ·  {}", widgets::fmt_time(e.duration_ms), ago(e.modified)),
            egui::FontId::proportional(11.0),
            p.secondary,
        );
        if resp.clicked() && !open {
            action = Action::Open(e.path.clone());
        }
        resp.context_menu(|ui| {
            if ui.button("Show in Finder").clicked() {
                super::macos::reveal(&e.path);
            }
        });
    }
    ui.separator();
    if ui.button("Open…").clicked()
        && let Some(f) = rfd::FileDialog::new().set_directory(dir).pick_folder()
    {
        action = Action::Open(f);
    }
    if ui.button("Show projects folder").clicked() {
        super::macos::open_folder(dir);
    }
    action
}

/// A small down chevron at the right end of a menu button (Inter has no ▾;
/// the label leaves room for it).
fn chevron(ui: &Ui, r: &egui::Response) {
    let c = egui::pos2(r.rect.right() - 13.0, r.rect.center().y + 1.0);
    let stroke = Stroke::new(1.5, pal(ui).secondary);
    ui.painter().line_segment([c + vec2(-4.0, -2.5), c], stroke);
    ui.painter().line_segment([c, c + vec2(4.0, -2.5)], stroke);
}

/// What the editor's title bar needs from the app: recording settings and
/// recent recordings. Its `action` is handled after the frame.
pub struct Chrome<'a> {
    pub settings: &'a mut Settings,
    pub devices: &'a mut Devices,
    pub projects: &'a [ProjectEntry],
    pub thumbs: &'a mut Thumbs,
    pub action: Action,
}

impl Chrome<'_> {
    fn set(&mut self, a: Action) {
        if !matches!(a, Action::None) {
            self.action = a;
        }
    }

    /// Back, then the title, which opens recent recordings.
    pub fn title_left(&mut self, ui: &mut Ui, title: &str, open: Option<&Path>) {
        if open.is_some() && ui.button("‹ Back").on_hover_text("Close this recording and set up a new one").clicked() {
            self.set(Action::Back);
        }
        ui.add_space(8.0);
        let dir = self.settings.dir();
        let mut a = Action::None;
        let r = ui.menu_button(RichText::new(format!("{title}      ")).font(theme::semibold(theme::BODY)), |ui| {
            a = recents(ui, self.projects, self.thumbs, open, &dir);
        });
        chevron(ui, &r.response);
        r.response.on_hover_text("Recent recordings");
        self.set(a);
    }

    /// Compact record controls: Record now, and a menu for the source and
    /// audio/camera (laid out right to left, so Record comes last).
    pub fn record_controls(&mut self, ui: &mut Ui) {
        let s = &mut *self.settings;
        let mut a = Action::None;
        let r = ui.menu_button(format!("{}      ", source_name(s.source)), |ui| {
            for k in [SourceKind::Display, SourceKind::Window, SourceKind::Area, SourceKind::Device] {
                if ui.selectable_label(s.source == k, source_name(k)).clicked() {
                    s.source = k;
                }
            }
            ui.separator();
            ui.checkbox(&mut s.system_audio, "System audio");
            ui.checkbox(&mut s.mic, "Microphone");
            ui.checkbox(&mut s.camera, "Camera");
            ui.separator();
            if ui.button("More options…").on_hover_text("Close this recording and show every recording option").clicked() {
                a = Action::Back;
            }
        });
        chevron(ui, &r.response);
        r.response.on_hover_text("What to record");
        let rec = egui::Button::new(RichText::new("●  Record").color(Color32::from_rgb(255, 69, 58)).font(theme::semibold(theme::BODY)));
        if ui.add(rec).on_hover_text(format!("{}  ({})", source_summary(s, self.devices), s.hotkeys.record_label())).clicked() {
            a = Action::Record;
        }
        self.set(a);
    }
}

/// The window with no recording open: the editor's layout with recording
/// setup in the inspector, a big Record button on the canvas and an empty
/// timeline.
pub fn start_view(ui: &mut Ui, s: &mut Settings, dev: &mut Devices, projects: &[ProjectEntry], thumbs: &mut Thumbs, permission_note: Option<&str>) -> Action {
    let mut chrome = Chrome { settings: s, devices: dev, projects, thumbs, action: Action::None };
    let p = pal(ui);
    egui::Panel::top("editor-toolbar")
        .exact_size(theme::TOOLBAR_H)
        .frame(egui::Frame::new().fill(p.window).inner_margin(egui::Margin { left: theme::TRAFFIC_LIGHTS_W as i8, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            super::toolbar_drag(ui);
            ui.horizontal_centered(|ui| {
                chrome.title_left(ui, "New recording", None);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(false, |ui| {
                        widgets::primary(ui, "Export");
                        ui.add_space(6.0);
                        let _ = ui.button("Presets");
                        ui.add_space(4.0);
                        widgets::icon_button(ui, widgets::Icon::Redo, false);
                        widgets::icon_button(ui, widgets::Icon::Undo, false);
                    });
                });
            });
        });

    egui::Panel::right("inspector")
        .exact_size(320.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(p.inspector).inner_margin(egui::Margin::symmetric(14, 10)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                // The editor's tabs, with Record active until there's a recording.
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    let pill = |label: &str, sel: bool| {
                        egui::Button::new(RichText::new(label).size(12.0).color(if sel { Color32::WHITE } else { p.text }))
                            .fill(if sel { theme::accent() } else { p.control })
                            .stroke(if sel { Stroke::NONE } else { Stroke::new(0.5, p.control_border) })
                            .corner_radius(CornerRadius::same(12))
                    };
                    ui.add(pill("Record", true));
                    for (_, label) in super::editor::Tab::ALL {
                        ui.add_enabled(false, pill(label, false)).on_disabled_hover_text("Record or open a recording to edit it");
                    }
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                let a = record_settings(ui, chrome.settings, chrome.devices, permission_note);
                chrome.set(a);
            });
        });

    egui::Panel::bottom("timeline")
        .exact_size(236.0)
        .frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin::symmetric(12, 8)))
        .show(ui, |ui| {
            super::timeline::empty(ui);
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(if ui.visuals().dark_mode { Color32::from_rgb(0x1c, 0x1c, 0x1e) } else { Color32::from_rgb(0xe4, 0xe4, 0xe7) }))
        .show(ui, |ui| {
            let s = &*chrome.settings;
            let mut a = Action::None;
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() / 2.0 - 130.0).max(16.0));
                let hk = s.hotkeys.record_label();
                if widgets::record_button(ui, 96.0, false).on_hover_text(format!("Start recording {hk}")).clicked() {
                    a = Action::Record;
                }
                ui.add_space(14.0);
                ui.label(RichText::new(format!("Record {}", source_name(s.source).to_lowercase())).font(theme::semibold(20.0)));
                ui.add_space(4.0);
                caption(ui, &source_summary(s, chrome.devices));
                let next = match s.source {
                    SourceKind::Area => "You'll drag to select the area next.",
                    SourceKind::Window if s.window.is_none() => "You'll pick the window on screen next.",
                    _ if s.countdown => "Recording starts after a 3 s countdown.",
                    _ => "",
                };
                if !next.is_empty() {
                    caption(ui, next);
                }
                caption(ui, &format!("Shortcut {hk}"));
                ui.add_space(18.0);
                if ui.button("Open a recording…").clicked()
                    && let Some(f) = rfd::FileDialog::new().set_directory(s.dir()).pick_folder()
                {
                    a = Action::Open(f);
                }
            });
            chrome.set(a);
        });
    chrome.action
}

fn ago(t: SystemTime) -> String {
    let s = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}
