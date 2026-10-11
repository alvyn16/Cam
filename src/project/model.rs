//! The editable project: style, timeline edits and export settings.
//! Stored as `project.json` inside a `.glide` package.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::engine::geom::{Rect, Vec2, v2};
use crate::engine::spring::SpringConfig;

pub const PROJECT_VERSION: u32 = 1;

/// sRGB colour with alpha, serialised as `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const BLACK: Color = Color([0, 0, 0, 255]);
    pub const WHITE: Color = Color([255, 255, 255, 255]);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color([r, g, b, 255])
    }
    pub fn to_f32(self) -> [f32; 4] {
        self.0.map(|c| c as f32 / 255.0)
    }
    pub fn hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
    }
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            6 => Some(Color([byte(0)?, byte(2)?, byte(4)?, 255])),
            8 => Some(Color([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            _ => None,
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}
impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad colour {s:?}")))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Background {
    /// One of the built-in generated wallpapers.
    Wallpaper { id: String },
    Gradient { angle: f64, stops: Vec<(Color, f64)> },
    Color { color: Color },
    Image { path: String },
}

impl Default for Background {
    fn default() -> Self {
        Background::Wallpaper { id: "aurora".into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub intensity: f64,
    /// Direction the light comes from, degrees (90 = from above).
    pub angle: f64,
    pub distance: f64,
    pub blur: f64,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow { intensity: 0.75, angle: 90.0, distance: 25.0, blur: 20.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Inset {
    /// Inner border thickness in recording pixels.
    pub padding: f64,
    pub color: Color,
    pub alpha: f64,
}

impl Default for Inset {
    fn default() -> Self {
        Inset { padding: 0.0, color: Color::BLACK, alpha: 0.5 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Aspect {
    /// Recording plus padding.
    #[default]
    Auto,
    Ratio { w: u32, h: u32 },
}

impl Aspect {
    pub const PRESETS: [(&'static str, Aspect); 7] = [
        ("Auto", Aspect::Auto),
        ("Wide 16:9", Aspect::Ratio { w: 16, h: 9 }),
        ("Square 1:1", Aspect::Ratio { w: 1, h: 1 }),
        ("Classic 4:3", Aspect::Ratio { w: 4, h: 3 }),
        ("Vertical 9:16", Aspect::Ratio { w: 9, h: 16 }),
        ("Tall 3:4", Aspect::Ratio { w: 3, h: 4 }),
        ("Portrait 4:5", Aspect::Ratio { w: 4, h: 5 }),
    ];
    pub fn ratio(self) -> Option<f64> {
        match self {
            Aspect::Auto => None,
            Aspect::Ratio { w, h } => Some(w as f64 / h as f64),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClickEffect {
    #[default]
    None,
    /// An expanding soft disc under the cursor.
    Circle,
    /// A shockwave that ripples the screen content.
    Ripple { strength: f64 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorSkin {
    /// The cursor images recorded from the system.
    #[default]
    System,
    /// Crisp vector cursors drawn by the renderer at any size.
    Vector,
    /// A soft touch-style circle.
    Touch,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorStyle {
    pub size: f64,
    pub hide: bool,
    pub skin: CursorSkin,
    /// None = follow the raw mouse exactly.
    pub smoothing: Option<SpringConfig>,
    /// How much horizontal motion tilts the cursor.
    pub tilt: f64,
    pub base_rotation: f64,
    /// Hide after this long without movement.
    pub hide_idle_ms: Option<f64>,
    /// Glide back to the starting position in the last N ms (seamless loops).
    pub loop_ms: Option<f64>,
    /// Freeze in the last N ms.
    pub freeze_ms: f64,
    /// Glide onto upcoming click points this early, removing jitter.
    pub click_snap_ms: f64,
    pub click_effect: ClickEffect,
}

impl Default for CursorStyle {
    fn default() -> Self {
        CursorStyle {
            size: 1.5,
            hide: false,
            skin: CursorSkin::System,
            smoothing: Some(SpringConfig::CURSOR_SMOOTH),
            tilt: 0.5,
            base_rotation: 0.0,
            hide_idle_ms: None,
            loop_ms: None,
            freeze_ms: 0.0,
            click_snap_ms: 500.0,
            click_effect: ClickEffect::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionBlur {
    pub amount: f64,
    pub cursor: f64,
    pub screen_move: f64,
    pub screen_zoom: f64,
}

impl Default for MotionBlur {
    fn default() -> Self {
        MotionBlur { amount: 1.0, cursor: 1.0, screen_move: 1.0, screen_zoom: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CameraAspect {
    #[default]
    Original,
    Square,
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraStyle {
    pub hide: bool,
    pub mirror: bool,
    /// Fraction of the safe frame the bubble may occupy.
    pub size: f64,
    /// Corner radius as a fraction of the bubble's short side (0.5 = round).
    pub roundness: f64,
    /// Position inside the safe frame, (0,0) top-left to (1,1) bottom-right.
    pub position: Vec2,
    pub aspect: CameraAspect,
    /// Bubble scale while the screen is zoomed in.
    pub zoom_scale: f64,
}

impl Default for CameraStyle {
    fn default() -> Self {
        CameraStyle {
            hide: false,
            mirror: false,
            size: 0.35,
            roundness: 0.25,
            position: v2(1.0, 1.0),
            aspect: CameraAspect::Original,
            zoom_scale: 0.7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutStyle {
    pub show: bool,
    pub single_keys: bool,
    pub size: f64,
}

impl Default for ShortcutStyle {
    fn default() -> Self {
        ShortcutStyle { show: false, single_keys: false, size: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionStyle {
    pub show: bool,
    pub size: f64,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        CaptionStyle { show: true, size: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioStyle {
    pub volume: f64,
    pub mute_mic: bool,
    pub mute_system: bool,
    /// Gentle denoise + level for the microphone track.
    pub enhance_mic: bool,
    pub music: Option<String>,
    pub music_volume: f64,
    pub click_sounds: bool,
    pub click_volume: f64,
}

impl Default for AudioStyle {
    fn default() -> Self {
        AudioStyle {
            volume: 1.0,
            mute_mic: false,
            mute_system: false,
            enhance_mic: true,
            music: None,
            music_volume: 0.05,
            click_sounds: false,
            click_volume: 0.25,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub background: Background,
    pub background_blur: f64,
    /// Space around the window, percent of the mean screen dimension.
    pub padding: f64,
    /// Window corner radius in recording pixels.
    pub corner_radius: f64,
    pub shadow: Shadow,
    pub inset: Inset,
    pub aspect: Aspect,
    /// Normalised crop of the recording.
    pub crop: Rect,
    pub cursor: CursorStyle,
    pub screen_spring: SpringConfig,
    pub motion_blur: MotionBlur,
    pub camera: CameraStyle,
    pub shortcuts: ShortcutStyle,
    pub captions: CaptionStyle,
    pub audio: AudioStyle,
    /// Zoom level used for new automatic zooms.
    pub default_zoom: f64,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            background: Background::default(),
            background_blur: 0.0,
            padding: 10.0,
            corner_radius: 12.0,
            shadow: Shadow::default(),
            inset: Inset::default(),
            aspect: Aspect::Auto,
            crop: Rect::UNIT,
            cursor: CursorStyle::default(),
            screen_spring: SpringConfig::SCREEN_FOCUSED,
            motion_blur: MotionBlur::default(),
            camera: CameraStyle::default(),
            shortcuts: ShortcutStyle::default(),
            captions: CaptionStyle::default(),
            audio: AudioStyle::default(),
            default_zoom: 2.0,
        }
    }
}

/// A kept piece of the recording. Gaps between slices are cuts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slice {
    pub id: u64,
    /// Source (recording) time range, ms.
    pub start: f64,
    pub end: f64,
    pub speed: f64,
    pub mic_volume: f64,
    pub system_volume: f64,
    pub hide_cursor: bool,
    /// Follow the raw mouse in this slice.
    pub no_smoothing: bool,
}

impl Default for Slice {
    fn default() -> Self {
        Slice {
            id: 0,
            start: 0.0,
            end: 0.0,
            speed: 1.0,
            mic_volume: 1.0,
            system_volume: 1.0,
            hide_cursor: false,
            no_smoothing: false,
        }
    }
}

impl Slice {
    pub fn source_len(&self) -> f64 {
        self.end - self.start
    }
    pub fn playback_len(&self) -> f64 {
        self.source_len() / self.speed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ZoomMode {
    /// Follow clusters of clicks and activity.
    #[default]
    Auto,
    /// Zoom on a fixed point (normalised in the cropped screen).
    Manual { target: Vec2 },
    /// Keep the live cursor in view.
    FollowMouse,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Zoom {
    pub id: u64,
    /// Source time range, ms.
    pub start: f64,
    pub end: f64,
    pub scale: f64,
    pub mode: ZoomMode,
    /// How strongly targets near an edge push the view to that edge.
    pub snap_edges: f64,
    /// Cut in/out instead of animating.
    pub instant: bool,
    pub disabled: bool,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom {
            id: 0,
            start: 0.0,
            end: 0.0,
            scale: 2.0,
            mode: ZoomMode::Auto,
            snap_edges: 0.25,
            instant: false,
            disabled: false,
        }
    }
}

impl Zoom {
    pub const MIN_LEN_MS: f64 = 1000.0;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutKind {
    /// Screen with the camera bubble.
    #[default]
    Both,
    ScreenOnly,
    CameraOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutRange {
    pub id: u64,
    pub start: f64,
    pub end: f64,
    pub kind: LayoutKind,
    pub camera_size: Option<f64>,
    pub camera_position: Option<Vec2>,
}

impl Default for LayoutRange {
    fn default() -> Self {
        LayoutRange { id: 0, start: 0.0, end: 0.0, kind: LayoutKind::Both, camera_size: None, camera_position: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timeline {
    pub slices: Vec<Slice>,
    pub zooms: Vec<Zoom>,
    pub layouts: Vec<LayoutRange>,
    next_id: u64,
}

impl Timeline {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    #[default]
    Mp4,
    Gif,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Quality {
    Studio,
    #[default]
    Social,
    Web,
    WebLow,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::Studio, Quality::Social, Quality::Web, Quality::WebLow];

    /// Encoder bits per pixel per frame.
    pub fn bits_per_pixel(self) -> f64 {
        match self {
            Quality::Studio => 0.3,
            Quality::Social => 0.05,
            Quality::Web => 0.0175,
            Quality::WebLow => 0.0075,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Quality::Studio => "Studio",
            Quality::Social => "Social media",
            Quality::Web => "Web",
            Quality::WebLow => "Web (low)",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    #[default]
    H264,
    Hevc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub format: ExportFormat,
    /// Output height in pixels; width follows the project's aspect ratio.
    pub height: u32,
    pub fps: u32,
    pub quality: Quality,
    pub codec: Codec,
    pub gif_loop: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            format: ExportFormat::Mp4,
            height: 1080,
            fps: 60,
            quality: Quality::Social,
            codec: Codec::H264,
            gif_loop: true,
        }
    }
}

impl ExportSettings {
    pub const MP4_FPS: [u32; 7] = [60, 50, 30, 25, 24, 20, 10];
    pub const GIF_FPS: [u32; 6] = [50, 30, 25, 20, 15, 10];
    pub const MP4_HEIGHTS: [u32; 3] = [720, 1080, 2160];
    pub const GIF_HEIGHTS: [u32; 3] = [480, 720, 1080];

    pub fn gif_default() -> Self {
        ExportSettings { format: ExportFormat::Gif, height: 480, fps: 15, quality: Quality::Studio, ..Default::default() }
    }

    /// Target video bitrate (bits/s) for an output size.
    pub fn bitrate(&self, width: u32, height: u32) -> u64 {
        let bpp = self.quality.bits_per_pixel();
        let base = width as f64 * height as f64 * self.fps as f64 * bpp;
        // HEVC reaches the same quality with ~40% fewer bits.
        let codec = if self.codec == Codec::Hevc { 0.6 } else { 1.0 };
        (base * codec).max(250_000.0) as u64
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub style: Style,
    pub timeline: Timeline,
    pub export: ExportSettings,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            version: PROJECT_VERSION,
            id: String::new(),
            name: "Untitled".into(),
            created_ms: 0,
            updated_ms: 0,
            style: Style::default(),
            timeline: Timeline::default(),
            export: ExportSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_round_trip() {
        let c = Color([1, 0xab, 0xff, 0x80]);
        assert_eq!(Color::parse(&c.hex()), Some(c));
        assert_eq!(Color::parse("#ffffff"), Some(Color::WHITE));
    }

    #[test]
    fn project_round_trips_through_json() {
        let mut p = Project::default();
        p.style.background = Background::Gradient { angle: 135.0, stops: vec![(Color::BLACK, 0.0), (Color::WHITE, 1.0)] };
        p.timeline.zooms.push(Zoom { mode: ZoomMode::Manual { target: v2(0.2, 0.3) }, ..Default::default() });
        let s = serde_json::to_string(&p).unwrap();
        let q: Project = serde_json::from_str(&s).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let p: Project = serde_json::from_str(r#"{"name":"x","style":{"padding":3}}"#).unwrap();
        assert_eq!(p.style.padding, 3.0);
        assert_eq!(p.style.corner_radius, 12.0);
        assert_eq!(p.export.fps, 60);
    }

    #[test]
    fn bitrate_formula() {
        let e = ExportSettings::default();
        assert_eq!(e.bitrate(1920, 1080), (1920.0 * 1080.0 * 60.0 * 0.05) as u64);
    }
}
