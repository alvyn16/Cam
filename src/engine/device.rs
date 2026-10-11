//! Device frames: a laptop, phone or tablet drawn around the screen from
//! simple rounded shapes. All measurements are fractions of the screen's
//! shorter side, so a frame scales with the window as it zooms.

use super::geom::{Rect, Vec2, v2};
use crate::project::model::{Color, DeviceFrame};

/// One filled rounded rect of a device frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub rect: Rect,
    pub radius: f64,
    /// Vertical gradient, top to bottom.
    pub top: Color,
    pub bottom: Color,
    pub border: Color,
    pub border_width: f64,
    /// Drawn over the screen (camera housing) instead of under it.
    pub over: bool,
}

/// Space the frame takes around the screen: left, top, right, bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Margins {
    pub l: f64,
    pub t: f64,
    pub r: f64,
    pub b: f64,
}

impl Margins {
    pub fn size(&self) -> Vec2 {
        v2(self.l + self.r, self.t + self.b)
    }
}

fn hex(c: u32) -> Color {
    Color([(c >> 16) as u8, (c >> 8) as u8, c as u8, 255])
}

const LAPTOP_BEZEL: f64 = 0.035;
const LAPTOP_CHIN: f64 = 0.045;
const LAPTOP_BASE_H: f64 = 0.035;
/// How far the base sticks out on each side, as a fraction of screen width.
const LAPTOP_BASE_OVERHANG: f64 = 0.06;
const PHONE_BEZEL: f64 = 0.05;
const PHONE_BUTTON: f64 = 0.012;
const TABLET_BEZEL: f64 = 0.06;

pub fn margins(device: DeviceFrame, screen: Vec2) -> Margins {
    let m = screen.x.min(screen.y);
    match device {
        DeviceFrame::None => Margins::default(),
        DeviceFrame::Laptop => {
            let side = LAPTOP_BEZEL * m + LAPTOP_BASE_OVERHANG * screen.x;
            Margins { l: side, t: LAPTOP_BEZEL * m, r: side, b: (LAPTOP_CHIN + LAPTOP_BASE_H) * m }
        }
        DeviceFrame::Phone => {
            let side = (PHONE_BEZEL + PHONE_BUTTON) * m;
            Margins { l: side, t: PHONE_BEZEL * m, r: side, b: PHONE_BEZEL * m }
        }
        DeviceFrame::Tablet => {
            let b = TABLET_BEZEL * m;
            Margins { l: b, t: b, r: b, b }
        }
    }
}

/// Corner radius of the screen inside the frame (None = keep the style's).
pub fn screen_radius(device: DeviceFrame, screen: Rect) -> Option<f64> {
    let m = screen.w.min(screen.h);
    match device {
        DeviceFrame::None => None,
        DeviceFrame::Laptop => Some(0.012 * m),
        DeviceFrame::Phone => Some(0.15 * m),
        DeviceFrame::Tablet => Some(0.035 * m),
    }
}

/// The device body's outline (for the drop shadow), excluding thin buttons.
pub fn body(device: DeviceFrame, screen: Rect) -> Option<(Rect, f64)> {
    parts(device, screen).into_iter().filter(|p| !p.over && p.border_width > 0.0).map(|p| (p.rect, p.radius)).reduce(|a, b| {
        let r = Rect::from_points(a.0.min().min(b.0.min()), a.0.max().max(b.0.max()));
        (r, a.1.max(b.1))
    })
}

/// Shapes making up the frame around `screen` (output px), back to front.
pub fn parts(device: DeviceFrame, s: Rect) -> Vec<Part> {
    let m = s.w.min(s.h);
    let px = (m / 1000.0).max(0.5);
    match device {
        DeviceFrame::None => vec![],
        DeviceFrame::Laptop => {
            let b = LAPTOP_BEZEL * m;
            let chin = LAPTOP_CHIN * m;
            let lid = Rect::new(s.x - b, s.y - b, s.w + 2.0 * b, s.h + b + chin);
            let over = LAPTOP_BASE_OVERHANG * s.w;
            let base_h = LAPTOP_BASE_H * m;
            let base = Rect::new(lid.x - over, lid.y + lid.h - base_h * 0.15, lid.w + 2.0 * over, base_h);
            let groove = Rect::new(s.center().x - 0.08 * s.w, base.y, 0.16 * s.w, base_h * 0.32);
            let cam = 0.007 * m;
            vec![
                Part { rect: lid, radius: 0.045 * m, top: hex(0x2c2c30), bottom: hex(0x1a1a1d), border: hex(0x77787d), border_width: 2.5 * px, over: false },
                Part { rect: base, radius: base_h * 0.45, top: hex(0xe6e7ea), bottom: hex(0x9a9ca2), border: hex(0x7d7f85), border_width: 1.0 * px, over: false },
                Part { rect: groove, radius: base_h * 0.16, top: hex(0xa3a5ab), bottom: hex(0xc4c6cb), border: hex(0x8b8d93), border_width: 0.0, over: false },
                Part {
                    rect: Rect::new(s.center().x - cam, s.y - b * 0.5 - cam, 2.0 * cam, 2.0 * cam),
                    radius: cam,
                    top: hex(0x0c0c0e),
                    bottom: hex(0x18181c),
                    border: hex(0x2d2d33),
                    border_width: 0.0,
                    over: false,
                },
            ]
        }
        DeviceFrame::Phone => {
            let b = PHONE_BEZEL * m;
            let body = Rect::new(s.x - b, s.y - b, s.w + 2.0 * b, s.h + 2.0 * b);
            let btn = PHONE_BUTTON * m;
            let button = |x: f64, y: f64, h: f64| Part {
                rect: Rect::new(x, y, btn + 2.0 * px, h),
                radius: btn * 0.5,
                top: hex(0x55565b),
                bottom: hex(0x3a3b3f),
                border: hex(0x55565b),
                border_width: 0.0,
                over: false,
            };
            let island_w = 0.3 * m;
            let island_h = 0.085 * m;
            vec![
                button(body.x - btn, body.y + 0.18 * body.h, 0.1 * body.h),
                button(body.x - btn, body.y + 0.3 * body.h, 0.1 * body.h),
                button(body.x + body.w - 2.0 * px, body.y + 0.24 * body.h, 0.14 * body.h),
                Part { rect: body, radius: 0.2 * m, top: hex(0x2e2e32), bottom: hex(0x19191c), border: hex(0x86878c), border_width: 4.0 * px, over: false },
                Part {
                    rect: Rect::new(s.center().x - island_w / 2.0, s.y + 0.03 * m, island_w, island_h),
                    radius: island_h / 2.0,
                    top: hex(0x050506),
                    bottom: hex(0x050506),
                    border: hex(0x050506),
                    border_width: 0.0,
                    over: true,
                },
            ]
        }
        DeviceFrame::Tablet => {
            let b = TABLET_BEZEL * m;
            let body = Rect::new(s.x - b, s.y - b, s.w + 2.0 * b, s.h + 2.0 * b);
            let cam = 0.008 * m;
            let cam_at = v2(s.center().x, s.y - b * 0.5);
            vec![
                Part { rect: body, radius: 0.075 * m, top: hex(0x2e2e32), bottom: hex(0x1b1b1e), border: hex(0x8a8b90), border_width: 3.0 * px, over: false },
                Part {
                    rect: Rect::new(cam_at.x - cam, cam_at.y - cam, 2.0 * cam, 2.0 * cam),
                    radius: cam,
                    top: hex(0x0c0c0e),
                    bottom: hex(0x18181c),
                    border: hex(0x2d2d33),
                    border_width: 0.0,
                    over: false,
                },
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_fit_inside_margins() {
        for d in [DeviceFrame::Laptop, DeviceFrame::Phone, DeviceFrame::Tablet] {
            let s = Rect::new(500.0, 400.0, 1600.0, 1000.0);
            let m = margins(d, s.size());
            let outer = Rect::new(s.x - m.l, s.y - m.t, s.w + m.l + m.r, s.h + m.t + m.b);
            for p in parts(d, s) {
                assert!(p.rect.x >= outer.x - 1e-6 && p.rect.y >= outer.y - 1e-6, "{d:?} {p:?}");
                assert!(p.rect.x + p.rect.w <= outer.x + outer.w + 1e-6, "{d:?} {p:?}");
                assert!(p.rect.y + p.rect.h <= outer.y + outer.h + 1e-6, "{d:?} {p:?}");
            }
            assert!(body(d, s).is_some());
        }
        assert!(parts(DeviceFrame::None, Rect::UNIT).is_empty());
        assert_eq!(margins(DeviceFrame::None, v2(10.0, 10.0)), Margins::default());
    }
}
