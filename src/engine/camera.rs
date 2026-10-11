//! Camera bubble placement and layout switching.

use super::geom::{Rect, Vec2, v2};
use crate::project::model::{CameraAspect, CameraStyle, LayoutKind, LayoutRange};

/// Safe margin around the bubble, as a fraction of the output's short side.
pub const SAFE_MARGIN: f64 = 0.05;

/// Layout in effect at source time `t`.
pub fn layout_at(layouts: &[LayoutRange], t: f64) -> Option<&LayoutRange> {
    layouts.iter().filter(|l| t >= l.start && t <= l.end).max_by(|a, b| a.start.total_cmp(&b.start))
}

pub fn kind_at(layouts: &[LayoutRange], t: f64) -> LayoutKind {
    layout_at(layouts, t).map_or(LayoutKind::Both, |l| l.kind)
}

/// Aspect ratio (w/h) of the bubble.
pub fn aspect(style: &CameraStyle, video: Vec2) -> f64 {
    match style.aspect {
        CameraAspect::Square => 1.0,
        CameraAspect::Vertical => 9.0 / 16.0,
        CameraAspect::Horizontal => 16.0 / 9.0,
        CameraAspect::Original => {
            if video.y > 0.0 {
                video.x / video.y
            } else {
                16.0 / 9.0
            }
        }
    }
}

/// Bubble rect in design pixels for a given size fraction and position.
pub fn bubble_rect(output: Vec2, aspect: f64, size: f64, position: Vec2) -> Rect {
    let m = output.min_elem() * SAFE_MARGIN;
    let frame = Rect::new(m, m, output.x - 2.0 * m, output.y - 2.0 * m);
    let fit = frame.size().shrink_to_aspect(aspect) * size.clamp(0.02, 1.0);
    let room = frame.size() - fit;
    let p = position.clamp(0.0, 1.0);
    Rect::new(frame.x + p.x * room.x, frame.y + p.y * room.y, fit.x, fit.y)
}

/// Bubble rect at a moment, given whether the screen is zoomed.
pub fn bubble_at(style: &CameraStyle, layouts: &[LayoutRange], output: Vec2, video: Vec2, t: f64, zoomed: bool) -> Rect {
    let l = layout_at(layouts, t).filter(|l| l.kind == LayoutKind::Both);
    let size = l.and_then(|l| l.camera_size).unwrap_or(style.size);
    let size = if zoomed { size * style.zoom_scale } else { size };
    let position = l.and_then(|l| l.camera_position).unwrap_or(style.position);
    bubble_rect(output, aspect(style, video), size, position)
}

/// Named corner positions for the inspector.
pub const CORNERS: [(&str, Vec2); 4] =
    [("Top left", v2(0.0, 0.0)), ("Top right", v2(1.0, 0.0)), ("Bottom left", v2(0.0, 1.0)), ("Bottom right", v2(1.0, 1.0))];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_right_bubble_respects_margin() {
        let out = v2(1920.0, 1080.0);
        let r = bubble_rect(out, 1.0, 0.35, v2(1.0, 1.0));
        let m = 1080.0 * SAFE_MARGIN;
        assert!((r.max().x - (1920.0 - m)).abs() < 1e-9);
        assert!((r.max().y - (1080.0 - m)).abs() < 1e-9);
        assert!((r.w - (1080.0 - 2.0 * m) * 0.35).abs() < 1e-9);
    }

    #[test]
    fn layouts_pick_latest() {
        let ls = [
            LayoutRange { start: 0.0, end: 5000.0, kind: LayoutKind::ScreenOnly, ..Default::default() },
            LayoutRange { start: 2000.0, end: 3000.0, kind: LayoutKind::CameraOnly, ..Default::default() },
        ];
        assert_eq!(kind_at(&ls, 1000.0), LayoutKind::ScreenOnly);
        assert_eq!(kind_at(&ls, 2500.0), LayoutKind::CameraOnly);
        assert_eq!(kind_at(&ls, 6000.0), LayoutKind::Both);
    }
}
