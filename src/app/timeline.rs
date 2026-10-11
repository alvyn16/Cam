//! Timeline: ruler, clip track (slices), zoom track, mask track and camera
//! layout track.
//! Everything is drawn in playback time; edits are stored in source time.

use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::editor::{Editor, Selection};
use super::theme::{self, pal};
use crate::project::model::{LayoutKind, Mask, MaskKind, Zoom, ZoomMode};

const RULER_H: f32 = 18.0;
const TRACK_H: f32 = 40.0;
const GAP: f32 = 6.0;
const EDGE: f32 = 7.0;

#[derive(Clone, Copy)]
enum Drag {
    SliceEdge { id: u64, left: bool },
    Zoom { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Layout { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Mask { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Playhead,
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Body,
    Left,
    Right,
}

fn zoom_color() -> Color32 {
    Color32::from_rgb(0x8b, 0x5c, 0xf6)
}
fn clip_color() -> Color32 {
    Color32::from_rgb(0xf5, 0x9e, 0x0b)
}
fn layout_color() -> Color32 {
    Color32::from_rgb(0x10, 0xb9, 0x81)
}
fn mask_color() -> Color32 {
    Color32::from_rgb(0xec, 0x48, 0x99)
}

/// Greedy lane assignment so overlapping ranges don't cover each other.
fn lanes(ranges: &[(f64, f64)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    order.sort_by(|&a, &b| ranges[a].0.total_cmp(&ranges[b].0));
    let mut ends: Vec<f64> = vec![];
    let mut out = vec![0; ranges.len()];
    for i in order {
        let (s, e) = ranges[i];
        let lane = ends.iter().position(|&end| end <= s).unwrap_or_else(|| {
            ends.push(f64::MIN);
            ends.len() - 1
        });
        ends[lane] = e;
        out[i] = lane;
    }
    out
}

pub fn ui(ui: &mut Ui, ed: &mut Editor) {
    let p = pal(ui);
    let has_cam = ed.rec.meta.camera.is_some();
    let tracks = if has_cam { 4 } else { 3 };
    let height = RULER_H + tracks as f32 * (TRACK_H + GAP) + 4.0;
    let dur = ed.duration().max(1.0);
    let width = (dur as f32 / 1000.0 * ed.px_per_sec).max(ui.available_width() - 4.0);
    let drag_id = ui.id().with("timeline-drag");
    egui::ScrollArea::horizontal().id_salt("timeline-scroll").auto_shrink(false).show(ui, |ui| {
        let (rect, bg) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
        let px_per_ms = ed.px_per_sec / 1000.0;
        let x_of = |pb: f64| rect.left() + pb as f32 * px_per_ms;
        let pb_of = |x: f32| ((x - rect.left()) / px_per_ms) as f64;
        let painter = ui.painter_at(rect);
        let map = ed.preview.scene.map.clone();

        // Ruler.
        let ruler = Rect::from_min_size(rect.min, vec2(width, RULER_H));
        let step_s = [0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| *s * ed.px_per_sec as f64 >= 60.0).unwrap_or(120.0);
        let mut t = 0.0;
        while t * 1000.0 <= dur {
            let x = x_of(t * 1000.0);
            painter.line_segment([pos2(x, ruler.bottom() - 5.0), pos2(x, ruler.bottom())], Stroke::new(1.0, p.tertiary));
            painter.text(pos2(x + 3.0, ruler.top() + 1.0), egui::Align2::LEFT_TOP, super::widgets::fmt_time(t * 1000.0), egui::FontId::proportional(10.0), p.secondary);
            t += step_s;
        }

        let track = |i: usize| Rect::from_min_size(pos2(rect.left(), rect.top() + RULER_H + 2.0 + i as f32 * (TRACK_H + GAP)), vec2(width, TRACK_H));
        let clip_row = track(0);
        let zoom_row = track(1);
        let mask_row = track(2);
        let layout_row = track(3);
        let track_bg = if ui.visuals().dark_mode { Color32::from_white_alpha(8) } else { Color32::from_black_alpha(8) };
        for r in [clip_row, zoom_row, mask_row].into_iter().chain(has_cam.then_some(layout_row)) {
            painter.rect_filled(r, CornerRadius::same(6), track_bg);
        }

        let pointer = ui.input(|i| i.pointer.interact_pos());
        let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
        let mut hover_cursor = None;

        // Clip track: one block per kept slice.
        for l in &map.links {
            let r = Rect::from_min_max(pos2(x_of(l.pb_start) + 1.0, clip_row.top()), pos2(x_of(l.pb_end) - 1.0, clip_row.bottom()));
            let slice = ed.project.timeline.slices.get(l.slice).copied();
            let id = slice.map_or(0, |s| s.id);
            let selected = ed.selection == Selection::Slice(id);
            let c = clip_color();
            painter.rect_filled(r, CornerRadius::same(6), c.gamma_multiply(if selected { 0.95 } else { 0.7 }));
            if selected {
                painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            let mut label = String::from("Clip");
            if (l.speed - 1.0).abs() > 1e-3 {
                label = format!("{:.2}×", l.speed).replace(".00", "");
            }
            if slice.is_some_and(|s| s.hide_cursor) {
                label.push_str(" · cursor hidden");
            }
            painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, label, theme::semibold(11.0), Color32::WHITE);
            if let Some(pos) = pointer
                && r.expand(2.0).contains(pos)
                && slice.is_some()
            {
                let near_l = (pos.x - r.left()).abs() < EDGE;
                let near_r = (pos.x - r.right()).abs() < EDGE;
                if near_l || near_r {
                    hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                }
                if bg.drag_started() && drag.is_none() && (near_l || near_r) {
                    drag = Some(Drag::SliceEdge { id, left: near_l });
                } else if bg.clicked() {
                    ed.selection = Selection::Slice(id);
                }
            }
        }

        // Voice-overs: thin bars along the bottom of the clip track.
        for vo in &ed.project.timeline.voiceovers {
            let a = map.to_playback_nearest(vo.start);
            let b = (a + vo.duration).min(map.duration());
            if b > a {
                let r = Rect::from_min_max(pos2(x_of(a) + 2.0, clip_row.bottom() - 7.0), pos2(x_of(b) - 2.0, clip_row.bottom() - 3.0));
                painter.rect_filled(r, CornerRadius::same(2), Color32::from_rgb(0x06, 0xb6, 0xd4));
            }
        }

        // Zoom track.
        let zooms = ed.project.timeline.zooms.clone();
        for z in &zooms {
            let (a, b) = (map.to_playback_nearest(z.start), map.to_playback_nearest(z.end));
            if b <= a {
                continue;
            }
            let r = Rect::from_min_max(pos2(x_of(a) + 1.0, zoom_row.top()), pos2(x_of(b) - 1.0, zoom_row.bottom()));
            let selected = ed.selection == Selection::Zoom(z.id);
            let c = if z.disabled { p.tertiary } else { zoom_color() };
            painter.rect_filled(r, CornerRadius::same(6), c.gamma_multiply(if selected { 0.95 } else { 0.7 }));
            if selected {
                painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            let mode = match z.mode {
                ZoomMode::Auto => "Auto",
                ZoomMode::Manual { .. } => "Manual",
                ZoomMode::FollowMouse => "Follow",
            };
            if r.width() > 40.0 {
                painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, format!("{:.1}× {mode}", z.scale), theme::semibold(11.0), Color32::WHITE);
            }
            if let Some(pos) = pointer
                && r.expand(2.0).contains(pos)
            {
                let part = if (pos.x - r.left()).abs() < EDGE {
                    Part::Left
                } else if (pos.x - r.right()).abs() < EDGE {
                    Part::Right
                } else {
                    Part::Body
                };
                if part != Part::Body {
                    hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                }
                if bg.drag_started() && drag.is_none() {
                    drag = Some(Drag::Zoom { id: z.id, part, grab_src: map.to_source(pb_of(pos.x)), orig: (z.start, z.end) });
                    ed.selection = Selection::Zoom(z.id);
                } else if bg.clicked() {
                    ed.selection = Selection::Zoom(z.id);
                }
            }
        }
        // Double-click empty zoom track to add a zoom there.
        if bg.double_clicked()
            && let Some(pos) = pointer
            && zoom_row.contains(pos)
            && !zooms.iter().any(|z| {
                let (a, b) = (map.to_playback_nearest(z.start), map.to_playback_nearest(z.end));
                pos.x >= x_of(a) && pos.x <= x_of(b)
            })
        {
            let src = map.to_source(pb_of(pos.x));
            let scale = ed.project.style.default_zoom;
            let end_limit = ed.rec.meta.duration_ms;
            ed.edit(|p| {
                let id = p.timeline.new_id();
                p.timeline.zooms.push(Zoom { id, start: src, end: (src + 2500.0).min(end_limit), scale, ..Default::default() });
            });
        }

        // Mask track: overlapping masks stack in lanes.
        let masks = ed.project.timeline.masks.clone();
        let lanes = lanes(&masks.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>());
        let lane_count = lanes.iter().copied().max().map_or(1, |n| n + 1) as f32;
        for (m, lane) in masks.iter().zip(&lanes) {
            let (a, b) = (map.to_playback_nearest(m.start), map.to_playback_nearest(m.end));
            if b <= a {
                continue;
            }
            let lane_h = mask_row.height() / lane_count;
            let top = mask_row.top() + *lane as f32 * lane_h;
            let r = Rect::from_min_max(pos2(x_of(a) + 1.0, top + 0.5), pos2(x_of(b) - 1.0, top + lane_h - 0.5));
            let selected = ed.selection == Selection::Mask(m.id);
            painter.rect_filled(r, CornerRadius::same(6), mask_color().gamma_multiply(if selected { 0.95 } else { 0.7 }));
            if selected {
                painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            let label = match m.kind {
                MaskKind::Blur => "Blur",
                MaskKind::Pixelate => "Pixelate",
                MaskKind::Highlight => "Highlight",
            };
            if r.width() > 40.0 {
                painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, label, theme::semibold(11.0), Color32::WHITE);
            }
            if let Some(pos) = pointer
                && r.expand(2.0).contains(pos)
            {
                let part = if (pos.x - r.left()).abs() < EDGE {
                    Part::Left
                } else if (pos.x - r.right()).abs() < EDGE {
                    Part::Right
                } else {
                    Part::Body
                };
                if part != Part::Body {
                    hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                }
                if bg.drag_started() && drag.is_none() {
                    drag = Some(Drag::Mask { id: m.id, part, grab_src: map.to_source(pb_of(pos.x)), orig: (m.start, m.end) });
                    ed.selection = Selection::Mask(m.id);
                } else if bg.clicked() {
                    ed.selection = Selection::Mask(m.id);
                }
            }
        }
        // Double-click empty mask track to add a mask there.
        if bg.double_clicked()
            && let Some(pos) = pointer
            && mask_row.contains(pos)
            && !masks.iter().any(|m| {
                let (a, b) = (map.to_playback_nearest(m.start), map.to_playback_nearest(m.end));
                pos.x >= x_of(a) && pos.x <= x_of(b)
            })
        {
            let src = map.to_source(pb_of(pos.x));
            let end_limit = ed.rec.meta.duration_ms;
            let mut new_id = 0;
            ed.edit(|p| {
                let id = p.timeline.new_id();
                new_id = id;
                p.timeline.masks.push(Mask { id, start: src, end: (src + 3000.0).min(end_limit), ..Default::default() });
            });
            ed.selection = Selection::Mask(new_id);
        }

        // Layout track.
        if has_cam {
            let layouts = ed.project.timeline.layouts.clone();
            for l in &layouts {
                let (a, b) = (map.to_playback_nearest(l.start), map.to_playback_nearest(l.end));
                if b <= a {
                    continue;
                }
                let r = Rect::from_min_max(pos2(x_of(a) + 1.0, layout_row.top()), pos2(x_of(b) - 1.0, layout_row.bottom()));
                let selected = ed.selection == Selection::Layout(l.id);
                painter.rect_filled(r, CornerRadius::same(6), layout_color().gamma_multiply(if selected { 0.95 } else { 0.7 }));
                if selected {
                    painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
                }
                let label = match l.kind {
                    LayoutKind::Both => "Screen + camera",
                    LayoutKind::ScreenOnly => "Screen only",
                    LayoutKind::CameraOnly => "Camera only",
                };
                painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, label, theme::semibold(11.0), Color32::WHITE);
                if let Some(pos) = pointer
                    && r.expand(2.0).contains(pos)
                {
                    let part = if (pos.x - r.left()).abs() < EDGE {
                        Part::Left
                    } else if (pos.x - r.right()).abs() < EDGE {
                        Part::Right
                    } else {
                        Part::Body
                    };
                    if bg.drag_started() && drag.is_none() {
                        drag = Some(Drag::Layout { id: l.id, part, grab_src: map.to_source(pb_of(pos.x)), orig: (l.start, l.end) });
                        ed.selection = Selection::Layout(l.id);
                    } else if bg.clicked() {
                        ed.selection = Selection::Layout(l.id);
                    }
                }
            }
        }

        // Clicking the ruler or empty space moves the playhead.
        if (bg.clicked() || bg.drag_started())
            && drag.is_none()
            && let Some(pos) = pointer
        {
            let on_item = clip_row.contains(pos) || ((zoom_row.contains(pos) || mask_row.contains(pos)) && ed.selection != Selection::None);
            if ruler.contains(pos) || !on_item {
                ed.seek(pb_of(pos.x));
                if bg.drag_started() {
                    drag = Some(Drag::Playhead);
                }
                if !ruler.contains(pos) && bg.clicked() && !zoom_row.contains(pos) && !mask_row.contains(pos) && !clip_row.contains(pos) {
                    ed.selection = Selection::None;
                }
            }
        }

        // Apply an ongoing drag.
        if let (Some(d), Some(pos)) = (drag, pointer)
            && bg.dragged()
        {
            let src = map.to_source(pb_of(pos.x).clamp(0.0, map.duration()));
            let dur_src = ed.rec.meta.duration_ms;
            match d {
                Drag::Playhead => ed.seek(pb_of(pos.x)),
                Drag::SliceEdge { id, left } => {
                    let raw = map.to_source(pb_of(pos.x).clamp(0.0, map.duration()));
                    let mut draft = ed.project.clone();
                    let slices = &mut draft.timeline.slices;
                    if let Some(i) = slices.iter().position(|s| s.id == id) {
                        let lo = if i > 0 { slices[i - 1].end } else { 0.0 };
                        let hi = if i + 1 < slices.len() { slices[i + 1].start } else { dur_src };
                        let s = &mut slices[i];
                        if left {
                            // Dragging a left edge past its current source start
                            // extends into the cut region before it.
                            let target = if pos.x < x_of(map.to_playback_nearest(s.start)) - 1.0 { s.start - (x_of(map.to_playback_nearest(s.start)) - pos.x) as f64 / px_per_ms as f64 * s.speed } else { raw };
                            s.start = target.clamp(lo, s.end - 200.0);
                        } else {
                            let target = if pos.x > x_of(map.to_playback_nearest(s.end)) + 1.0 { s.end + (pos.x - x_of(map.to_playback_nearest(s.end))) as f64 / px_per_ms as f64 * s.speed } else { raw };
                            s.end = target.clamp(s.start + 200.0, hi);
                        }
                    }
                    ed.commit(draft);
                }
                Drag::Zoom { id, part, grab_src, orig } => {
                    let delta = src - grab_src;
                    let mut draft = ed.project.clone();
                    if let Some(z) = draft.timeline.zooms.iter_mut().find(|z| z.id == id) {
                        match part {
                            Part::Body => {
                                let len = orig.1 - orig.0;
                                z.start = (orig.0 + delta).clamp(0.0, (dur_src - len).max(0.0));
                                z.end = z.start + len;
                            }
                            Part::Left => z.start = (orig.0 + delta).clamp(0.0, orig.1 - Zoom::MIN_LEN_MS / 2.0),
                            Part::Right => z.end = (orig.1 + delta).clamp(orig.0 + Zoom::MIN_LEN_MS / 2.0, dur_src),
                        }
                    }
                    ed.commit(draft);
                }
                Drag::Mask { id, part, grab_src, orig } => {
                    let delta = src - grab_src;
                    let mut draft = ed.project.clone();
                    if let Some(m) = draft.timeline.masks.iter_mut().find(|m| m.id == id) {
                        match part {
                            Part::Body => {
                                let len = orig.1 - orig.0;
                                m.start = (orig.0 + delta).clamp(0.0, (dur_src - len).max(0.0));
                                m.end = m.start + len;
                            }
                            Part::Left => m.start = (orig.0 + delta).clamp(0.0, orig.1 - Mask::MIN_LEN_MS),
                            Part::Right => m.end = (orig.1 + delta).clamp(orig.0 + Mask::MIN_LEN_MS, dur_src),
                        }
                    }
                    ed.commit(draft);
                }
                Drag::Layout { id, part, grab_src, orig } => {
                    let delta = src - grab_src;
                    let mut draft = ed.project.clone();
                    if let Some(l) = draft.timeline.layouts.iter_mut().find(|l| l.id == id) {
                        match part {
                            Part::Body => {
                                let len = orig.1 - orig.0;
                                l.start = (orig.0 + delta).clamp(0.0, (dur_src - len).max(0.0));
                                l.end = l.start + len;
                            }
                            Part::Left => l.start = (orig.0 + delta).clamp(0.0, orig.1 - 300.0),
                            Part::Right => l.end = (orig.1 + delta).clamp(orig.0 + 300.0, dur_src),
                        }
                    }
                    ed.commit(draft);
                }
            }
        }
        if bg.drag_stopped() {
            drag = None;
        }
        ui.data_mut(|d| match drag {
            Some(v) => {
                d.insert_temp(drag_id, v);
            }
            None => {
                d.remove::<Drag>(drag_id);
            }
        });
        if let Some(c) = hover_cursor {
            ui.ctx().set_cursor_icon(c);
        }

        // Track labels and playhead.
        for (r, name) in [(clip_row, "Clips"), (zoom_row, "Zoom"), (mask_row, "Masks")].into_iter().chain(has_cam.then_some((layout_row, "Camera"))) {
            if map.links.is_empty() || name != "Clips" {
                painter.text(r.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, name, egui::FontId::proportional(10.0), p.tertiary);
            }
        }
        let x = x_of(ed.playhead);
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(2.0, theme::accent()));
        painter.add(egui::Shape::convex_polygon(
            vec![Pos2::new(x - 6.0, rect.top()), Pos2::new(x + 6.0, rect.top()), Pos2::new(x, rect.top() + 8.0)],
            theme::accent(),
            Stroke::NONE,
        ));
        if ed.is_playing() {
            ui.scroll_to_rect(Rect::from_center_size(pos2(x, rect.center().y), vec2(200.0, 10.0)), None);
        }
    });
    let _ = RichText::new("");
}

#[cfg(test)]
mod tests {
    #[test]
    fn overlapping_ranges_get_separate_lanes() {
        assert_eq!(super::lanes(&[(0.0, 5.0), (1.0, 2.0), (5.0, 6.0), (1.5, 7.0)]), vec![0, 1, 0, 2]);
    }
}
