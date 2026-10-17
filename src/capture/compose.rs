//! Camera and screen composition parameters on Windows.

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    Rounded,
    Circle,
    Rect,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scene {
    Screen,
    #[default]
    ScreenCam,
    Cam,
}

impl Scene {
    pub const ALL: [Scene; 3] = [Scene::Screen, Scene::ScreenCam, Scene::Cam];

    pub fn label(self) -> &'static str {
        match self {
            Scene::Screen => "Screen",
            Scene::ScreenCam => "Screen + Camera",
            Scene::Cam => "Camera",
        }
    }

    pub fn uses_cam(self) -> bool {
        self != Scene::Screen
    }
}

pub fn fit(sw: f64, sh: f64, dw: f64, dh: f64, cover: bool) -> (f64, f64, f64) {
    let (a, b) = (dw / sw, dh / sh);
    let s = if cover { a.max(b) } else { a.min(b) };
    (s, (dw - sw * s) / 2.0, (dh - sh * s) / 2.0)
}

#[derive(Clone, Debug)]
pub struct PipSpec {
    pub size: f64,
    pub corner: Corner,
    pub shape: Shape,
    pub mirror: bool,
}

pub struct Compositor;
