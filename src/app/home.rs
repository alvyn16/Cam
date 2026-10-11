//! Home: start a recording and browse projects.

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

#[derive(Default)]
pub struct Devices {
    pub displays: Vec<(u32, String)>,
    pub windows: Vec<(u32, String)>,
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
                    ws.into_iter()
                        .filter(|w| !w.title.is_empty() && w.app != "Glide")
                        .map(|w| (w.id, format!("{} — {}", w.app, w.title)))
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
#[derive(Default)]
pub struct Thumbs {
    pub textures: HashMap<PathBuf, egui::TextureHandle>,
    pending: Arc<Mutex<Vec<(PathBuf, crate::render::Image)>>>,
    requested: std::collections::HashSet<PathBuf>,
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
            let (p, pending) = (path.to_path_buf(), self.pending.clone());
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                if let Some(img) = thumbnail(&p) {
                    pending.lock().unwrap().push((p, img));
                    ctx.request_repaint();
                }
            });
        }
        None
    }

    pub fn forget(&mut self, path: &Path) {
        self.textures.remove(path);
        self.requested.remove(path);
    }
}

/// Load `thumb.png` or render one from a frame 1 s in.
fn thumbnail(path: &Path) -> Option<crate::render::Image> {
    let thumb = path.join("thumb.png");
    if let Ok(img) = image::open(&thumb) {
        let img = img.to_rgba8();
        return Some(crate::render::Image { width: img.width(), height: img.height(), rgba: img.into_raw() });
    }
    objc2::rc::autoreleasepool(|_| {
        let pkg = Package::new(path);
        let (p, rec) = pkg.load().ok()?;
        let mut scene = crate::engine::scene::Scene::new(p.clone(), &rec, 30.0);
        let t = (scene.duration() * 0.25).min(1500.0);
        let frame = scene.frame(t, 270.0);
        let gpu = crate::render::Gpu::headless().ok()?;
        let mut r = crate::render::Renderer::new(gpu);
        r.prepare(&p.style, &rec, Some(path));
        let mut src = crate::media::FrameSource::open(&pkg.rec(&rec.meta.screen.file)).ok()?;
        if let Some(f) = src.frame_at(frame.source_ms) {
            f.with_bgra(|v| r.set_screen(v));
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
}

pub fn ui(
    ui: &mut Ui,
    s: &mut Settings,
    dev: &mut Devices,
    projects: &mut Vec<ProjectEntry>,
    thumbs: &mut Thumbs,
    permission_note: Option<&str>,
) -> Action {
    let mut action = Action::None;
    let p = pal(ui);
    egui::Panel::top("home-toolbar")
        .exact_size(theme::TOOLBAR_H)
        .frame(egui::Frame::new().fill(p.window).inner_margin(egui::Margin { left: theme::TRAFFIC_LIGHTS_W as i8, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            super::toolbar_drag(ui);
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("Glide").font(theme::semibold(15.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Open…").clicked()
                        && let Some(f) = rfd::FileDialog::new().set_directory(s.dir()).pick_folder()
                    {
                        action = Action::Open(f);
                    }
                    if ui.button("Projects folder").clicked() {
                        super::macos::open_folder(&s.dir());
                    }
                });
            });
        });

    egui::Panel::left("new-recording")
        .exact_size(340.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin::symmetric(18, 14)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.label(RichText::new("New recording").font(theme::semibold(17.0)));
                ui.add_space(10.0);
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
                        let label = s.window.and_then(|w| dev.windows.iter().find(|d| d.0 == w)).map_or("Choose a window".into(), |d| d.1.clone());
                        egui::ComboBox::from_id_salt("window").width(290.0).selected_text(label).show_ui(ui, |ui| {
                            for (id, name) in &dev.windows {
                                ui.selectable_value(&mut s.window, Some(*id), name);
                            }
                        });
                        if ui.small_button("Refresh windows").clicked() {
                            *dev = Devices::load();
                        }
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
                ui.add_space(18.0);
                ui.vertical_centered(|ui| {
                    let hk = s.hotkeys.record_label();
                    if widgets::record_button(ui, 64.0, false).on_hover_text(format!("Start recording {hk}")).clicked() {
                        action = Action::Record;
                    }
                    ui.add_space(4.0);
                    caption(ui, &format!("Start recording  {hk}"));
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
            });
        });

    egui::CentralPanel::default().frame(egui::Frame::new().fill(p.window).inner_margin(egui::Margin::same(20))).show(ui, |ui| {
        ui.label(RichText::new("Projects").font(theme::semibold(17.0)));
        ui.add_space(10.0);
        if projects.is_empty() {
            caption(ui, "Your recordings will appear here.");
            return;
        }
        let mut remove: Option<PathBuf> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(16.0, 16.0);
                for e in projects.iter() {
                    let card = vec2(232.0, 180.0);
                    let (rect, resp) = ui.allocate_exact_size(card, Sense::click());
                    let hover = resp.hovered();
                    let painter = ui.painter_at(rect.expand(4.0));
                    let thumb_rect = egui::Rect::from_min_size(rect.min, vec2(card.x, 131.0));
                    painter.rect_filled(thumb_rect, CornerRadius::same(8), if ui.visuals().dark_mode { Color32::from_white_alpha(10) } else { Color32::from_black_alpha(10) });
                    if let Some(tex) = thumbs.get(ui.ctx(), &e.path) {
                        painter.image(tex, thumb_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    painter.rect_stroke(thumb_rect, CornerRadius::same(8), Stroke::new(if hover { 2.0 } else { 0.5 }, if hover { theme::accent() } else { p.control_border }), egui::StrokeKind::Outside);
                    painter.text(thumb_rect.left_bottom() + vec2(2.0, 8.0), egui::Align2::LEFT_TOP, &e.name, theme::semibold(12.5), p.text);
                    painter.text(
                        thumb_rect.left_bottom() + vec2(2.0, 27.0),
                        egui::Align2::LEFT_TOP,
                        format!("{}  ·  {}", widgets::fmt_time(e.duration_ms), ago(e.modified)),
                        egui::FontId::proportional(11.0),
                        p.secondary,
                    );
                    if resp.clicked() {
                        action = Action::Open(e.path.clone());
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Open").clicked() {
                            action = Action::Open(e.path.clone());
                            ui.close();
                        }
                        if ui.button("Show in Finder").clicked() {
                            super::macos::reveal(&e.path);
                            ui.close();
                        }
                        if ui.button("Move to Trash").clicked() {
                            remove = Some(e.path.clone());
                            ui.close();
                        }
                    });
                }
            });
        });
        if let Some(r) = remove {
            super::macos::trash(&r);
            thumbs.forget(&r);
            projects.retain(|e| e.path != r);
        }
    });
    action
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
