//! Small UI building blocks in the macOS/paper-rs style.

use egui::{Color32, CornerRadius, Response, RichText, Sense, Stroke, Ui, vec2};

use super::theme::{self, pal};

/// A titled inspector section.
pub fn section(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).font(theme::semibold(theme::SECONDARY + 1.0)).color(pal(ui).secondary));
    ui.add_space(4.0);
    add(ui);
    ui.add_space(8.0);
    ui.separator();
}

/// Label on the left, control on the right.
pub fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

/// A labelled slider; returns true when changed.
pub fn slider(ui: &mut Ui, label: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, suffix: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add(
                egui::Slider::new(v, range)
                    .show_value(true)
                    .suffix(suffix)
                    .max_decimals(2)
                    .trailing_fill(true),
            );
            changed = r.changed();
        });
    });
    changed
}

/// macOS-style switch; returns true when toggled.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(30.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = pal(ui);
    let off = if ui.visuals().dark_mode { Color32::from_rgb(0x4a, 0x4a, 0x4d) } else { Color32::from_rgb(0xd9, 0xd9, 0xdc) };
    let bg = lerp_color(off, theme::accent(), t);
    ui.painter().rect_filled(rect, CornerRadius::same(9), bg);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    ui.painter().circle(egui::pos2(x, rect.center().y), 7.0, Color32::WHITE, Stroke::new(0.5, p.control_border));
    resp
}

/// Label + switch on one row.
pub fn toggle_row(ui: &mut Ui, label: &str, on: &mut bool) -> bool {
    let mut changed = false;
    row(ui, label, |ui| changed = switch(ui, on).changed());
    changed
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Segmented control. Returns true when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    let p = pal(ui);
    egui::Frame::new()
        .fill(if ui.visuals().dark_mode { Color32::from_rgb(0x1f, 0x1f, 0x21) } else { Color32::from_rgb(0xe3, 0xe3, 0xe6) })
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Inside right-aligned rows items flow right to left; add them in
            // reverse so they still read left to right.
            let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
            ui.horizontal(|ui| {
                let order: Vec<&(T, &str)> = if rtl { options.iter().rev().collect() } else { options.iter().collect() };
                for (v, label) in order {
                    let sel = *value == *v;
                    let text = RichText::new(*label).size(theme::SECONDARY + 1.0);
                    let b = egui::Button::new(text)
                        .fill(if sel { if ui.visuals().dark_mode { Color32::from_rgb(0x5c, 0x5c, 0x60) } else { Color32::WHITE } } else { Color32::TRANSPARENT })
                        .stroke(if sel { Stroke::new(0.5, p.control_border) } else { Stroke::NONE })
                        .corner_radius(CornerRadius::same(5))
                        .min_size(vec2(0.0, 20.0));
                    if ui.add(b).clicked() && !sel {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Primary (accent) button.
pub fn primary(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).font(theme::semibold(theme::BODY)))
            .fill(theme::accent())
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(7))
            .min_size(vec2(0.0, 26.0)),
    )
}

/// Big round record button.
pub fn record_button(ui: &mut Ui, size: f32, recording: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let hover = resp.hovered();
    let c = rect.center();
    ui.painter().circle_filled(c, size / 2.0, if hover { Color32::from_rgb(255, 80, 70) } else { Color32::from_rgb(255, 59, 48) });
    if recording {
        let s = size * 0.28;
        ui.painter().rect_filled(egui::Rect::from_center_size(c, vec2(s, s)), CornerRadius::same(3), Color32::WHITE);
    } else {
        ui.painter().circle_filled(c, size * 0.18, Color32::WHITE);
    }
    resp
}

/// sRGB colour picker on a project colour.
pub fn color_edit(ui: &mut Ui, c: &mut crate::project::model::Color) -> bool {
    let mut col = Color32::from_rgba_unmultiplied(c.0[0], c.0[1], c.0[2], c.0[3]);
    let changed = egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque).changed();
    if changed {
        let [r, g, b, a] = col.to_srgba_unmultiplied();
        c.0 = [r, g, b, a];
    }
    changed
}

/// Small secondary caption text.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(theme::SECONDARY).color(pal(ui).secondary));
}

/// Format milliseconds as m:ss.t
pub fn fmt_time(ms: f64) -> String {
    let s = (ms / 1000.0).max(0.0);
    format!("{}:{:04.1}", (s / 60.0) as u64, s % 60.0)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Undo,
    Redo,
    Restart,
    Close,
    Pause,
    Play,
}

/// Small borderless toolbar button with a vector icon (Inter has no arrows).
pub fn icon_button(ui: &mut Ui, icon: Icon, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 22.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = pal(ui);
    if enabled && resp.hovered() {
        let fill = if resp.is_pointer_button_down_on() { p.pressed } else { p.hover };
        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    }
    let color = if enabled { p.text } else { p.tertiary };
    let stroke = Stroke::new(1.5, color);
    let c = rect.center();
    let painter = ui.painter();
    let arc = |from: f32, to: f32, r: f32, cy: f32| -> Vec<egui::Pos2> {
        (0..=16)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 16.0;
                egui::pos2(c.x + r * a.cos(), cy - r * a.sin())
            })
            .collect()
    };
    use std::f32::consts::PI;
    match icon {
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let mut pts = vec![egui::pos2(c.x + flip * 5.5, c.y + 5.0)];
            pts.extend(arc(0.0, PI, 5.5, c.y + 1.5).into_iter().map(|q| egui::pos2(c.x + (q.x - c.x) * flip, q.y)));
            let end = *pts.last().unwrap();
            painter.add(egui::Shape::line(pts, stroke));
            painter.line_segment([end, egui::pos2(end.x - 3.0, end.y - 3.0)], stroke);
            painter.line_segment([end, egui::pos2(end.x + 3.0, end.y - 3.0)], stroke);
        }
        Icon::Restart => {
            let pts = arc(PI * 0.35, PI * 2.15, 6.0, c.y);
            let end = pts[0];
            painter.add(egui::Shape::line(pts, stroke));
            painter.line_segment([end, egui::pos2(end.x - 3.5, end.y - 0.5)], stroke);
            painter.line_segment([end, egui::pos2(end.x + 0.5, end.y + 3.5)], stroke);
        }
        Icon::Close => {
            let d = 4.5;
            painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
            painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
        }
        Icon::Pause => {
            for dx in [-3.0, 3.0] {
                painter.rect_filled(egui::Rect::from_center_size(c + vec2(dx, 0.0), vec2(3.0, 11.0)), CornerRadius::same(1), color);
            }
        }
        Icon::Play => {
            let pts = vec![c + vec2(-4.0, -6.0), c + vec2(6.0, 0.0), c + vec2(-4.0, 6.0)];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
    }
    resp
}

/// Outline of a camera bubble filling `rect`, clockwise: a rounded rect with
/// corner radius `radius`, or a superellipse when `n` > 0.
pub fn bubble_outline(rect: egui::Rect, radius: f32, n: f32) -> Vec<egui::Pos2> {
    let c = rect.center();
    let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
    if n > 0.0 {
        return (0..128)
            .map(|i| {
                let t = i as f32 / 128.0 * std::f32::consts::TAU;
                let (ct, st) = (t.cos(), t.sin());
                c + vec2(ct.signum() * ct.abs().powf(2.0 / n) * hw, st.signum() * st.abs().powf(2.0 / n) * hh)
            })
            .collect();
    }
    let r = radius.clamp(0.0, hw.min(hh));
    let mut pts = Vec::with_capacity(4 * 17);
    // Corner centres, starting top right and going clockwise (y down).
    for (k, (sx, sy)) in [(1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)].into_iter().enumerate() {
        let cc = c + vec2(sx * (hw - r), sy * (hh - r));
        let a0 = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::FRAC_PI_2;
        for j in 0..=16 {
            let a = a0 + j as f32 / 16.0 * std::f32::consts::FRAC_PI_2;
            pts.push(cc + vec2(a.cos(), a.sin()) * r);
        }
    }
    pts
}

/// A small icon of a camera shape (with the current roundness).
fn shape_icon(ui: &Ui, rect: egui::Rect, c: &crate::project::model::CameraStyle, color: Color32) {
    let (w, h) = if c.shape.is_square() { (16.0, 16.0) } else { (22.0, 13.0) };
    let r = egui::Rect::from_center_size(rect.center(), vec2(w, h));
    let pts = bubble_outline(r, c.radius(w as f64, h as f64) as f32, c.squircle_exponent() as f32);
    ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.5, color)));
}

/// The camera bubble's look and position, as in Rec: shape, size,
/// roundness, border, corner, shadow and mirror. Returns true when changed.
pub fn camera_look(ui: &mut Ui, c: &mut crate::project::model::CameraStyle) -> bool {
    use crate::project::model::{CameraShape, CameraStyle};
    let before = *c;
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let bw = ((ui.available_width() - 16.0) / 5.0).max(30.0);
        for shape in CameraShape::ALL {
            let sel = c.shape == shape;
            let (rect, resp) = ui.allocate_exact_size(vec2(bw, 30.0), Sense::click());
            let fill = if sel { theme::accent() } else if resp.hovered() { p.hover } else { p.control };
            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
            shape_icon(ui, rect, &CameraStyle { shape, ..*c }, if sel { Color32::WHITE } else { p.text });
            if resp.on_hover_text(shape.label()).clicked() {
                c.shape = shape;
            }
        }
    });
    ui.add_space(4.0);
    let pct = |ui: &mut Ui, label: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, scale: f64, enabled: bool| {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut x = *v * scale;
                let r = (*range.start() * scale)..=(*range.end() * scale);
                if ui.add_enabled(enabled, egui::Slider::new(&mut x, r).suffix(" %").integer().trailing_fill(true)).changed() {
                    *v = x / scale;
                }
            });
        });
    };
    pct(ui, "Size", &mut c.size, 0.1..=0.8, 100.0, true);
    // Roundness 0..0.5 of the short side shows as 0..100 %.
    pct(ui, "Roundness", &mut c.roundness, 0.0..=0.5, 200.0, c.shape.uses_roundness());
    ui.horizontal(|ui| {
        ui.label("Border");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(c.border > 0.0, |ui| color_edit(ui, &mut c.border_color));
            ui.add(egui::Slider::new(&mut c.border, 0.0..=20.0).suffix(" px").integer().trailing_fill(true));
        });
    });
    row(ui, "Position", |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        // Right to left: the last corner is drawn first.
        for (label, tip, pos) in [("↘", "Bottom right", (1.0, 1.0)), ("↙", "Bottom left", (0.0, 1.0)), ("↗", "Top right", (1.0, 0.0)), ("↖", "Top left", (0.0, 0.0))] {
            let sel = (c.position.x, c.position.y) == pos;
            if ui.add(egui::Button::selectable(sel, label).min_size(vec2(26.0, 22.0))).on_hover_text(tip).clicked() {
                c.position = crate::engine::geom::v2(pos.0, pos.1);
            }
        }
    });
    toggle_row(ui, "Shadow", &mut c.shadow);
    toggle_row(ui, "Mirror", &mut c.mirror);
    *c != before
}
