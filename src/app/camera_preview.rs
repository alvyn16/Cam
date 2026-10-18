//! Floating camera monitor. Frames come from the recorder, never a second device.
use super::{home, theme, widgets};
use crate::{capture::grab::Image, project::model::CameraStyle};
use egui::{Color32, Sense, Stroke, TextureHandle, Ui, pos2, vec2};

#[derive(Default)]
pub(super) struct Preview {
    texture: Option<TextureHandle>,
    pub hidden: bool,
    pub frames: usize,
}

impl Preview {
    pub fn draw(&mut self, ui: &mut Ui, frame: Option<&Image>, style: &CameraStyle, paused: bool) {
        if let Some(frame) = frame {
            let rgba = frame
                .bgra
                .chunks_exact(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect::<Vec<_>>();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &rgba,
            );
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture = Some(ui.ctx().load_texture(
                    "recording-camera",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
            self.frames += 1;
        }
        let pal = theme::palette(ui.visuals().dark_mode);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(pal.popover)
                    .stroke(Stroke::new(1.0, pal.control_border))
                    .corner_radius(16)
                    .inner_margin(12),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Camera").font(theme::semibold(13.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("×")
                            .on_hover_text("Hide preview (camera keeps recording)")
                            .clicked()
                        {
                            self.hidden = true;
                        }
                    });
                });
                ui.add_space(8.0);
                let (rect, response) = ui.allocate_exact_size(vec2(208.0, 208.0), Sense::drag());
                if response.drag_started() {
                    super::DRAG.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                response
                    .on_hover_cursor(egui::CursorIcon::Grab)
                    .on_hover_text("Drag to move the camera preview");
                if let Some(texture) = &self.texture {
                    let size = texture.size_vec2();
                    let crop = crate::engine::scene::centre_crop((size.x / size.y) as f64, 1.0);
                    let radius = style.radius(rect.width() as f64, rect.height() as f64) as f32;
                    let points =
                        widgets::bubble_outline(rect, radius, style.squircle_exponent() as f32);
                    let uv = |p: egui::Pos2| {
                        let x = (p.x - rect.left()) / rect.width();
                        let y = (p.y - rect.top()) / rect.height();
                        pos2(
                            crop.x as f32
                                + (if style.mirror { 1.0 - x } else { x }) * crop.w as f32,
                            crop.y as f32 + y * crop.h as f32,
                        )
                    };
                    ui.painter().add(home::outline_mesh(
                        &points,
                        rect.center(),
                        texture.id(),
                        Color32::WHITE,
                        uv,
                    ));
                } else {
                    ui.painter().rect_filled(rect, 12, pal.control);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Waiting for camera…",
                        egui::FontId::proportional(12.0),
                        pal.secondary,
                    );
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(if paused {
                        "Paused · preview stays live"
                    } else {
                        "Preview only · not in screen capture"
                    })
                    .size(11.0)
                    .color(pal.secondary),
                );
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// Configure before showing: stay above other apps without taking keyboard focus.
pub(super) fn configure(window: &winit::window::Window) -> anyhow::Result<()> {
    use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
        anyhow::bail!("Camera preview requires a Windows window")
    };
    unsafe {
        let hwnd = HWND(handle.hwnd.get() as _);
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongW(
            hwnd,
            GWL_EXSTYLE,
            (style | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as i32,
        );
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)?;
    }
    Ok(())
}
