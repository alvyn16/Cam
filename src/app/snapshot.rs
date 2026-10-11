//! Render the real UI headlessly to a PNG (no window, no display access):
//! used to check layouts in CI and on machines where the app can't be shown.

use std::path::Path;

use anyhow::Result;

use super::editor::{Editor, Selection, Tab};
use super::home::{self, Devices, Settings, Thumbs};
use super::theme;
use crate::render::Gpu;
use crate::render::gpu::read_texture;

pub struct Shot<'a> {
    /// Editor for this project, or the home view when None.
    pub project: Option<&'a Path>,
    pub tab: Option<Tab>,
    /// Select the first "zoom", "mask" or "slice".
    pub select: Option<String>,
    pub export_sheet: bool,
    /// Show the just-recorded banner.
    pub fresh: bool,
    pub dark: bool,
    pub playhead_ms: f64,
    pub size: (f32, f32),
}

pub fn render(shot: &Shot, out_file: &Path) -> Result<()> {
    let gpu = Gpu::headless()?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());
    let ctx = egui::Context::default();
    ctx.set_theme(if shot.dark { egui::Theme::Dark } else { egui::Theme::Light });
    theme::apply(&ctx);
    let ppp = 2.0f32;
    let (w, h) = shot.size;

    let mut editor = match shot.project {
        Some(p) => Some(Editor::open(p, gpu.clone())?),
        None => None,
    };
    if let Some(ed) = &mut editor {
        if let Some(t) = shot.tab {
            ed.tab = t;
        }
        let tl = &ed.project.timeline;
        ed.selection = match shot.select.as_deref() {
            Some("zoom") => tl.zooms.first().map_or(Selection::None, |z| Selection::Zoom(z.id)),
            Some("mask") => tl.masks.first().map_or(Selection::None, |m| Selection::Mask(m.id)),
            Some("slice") => tl.slices.first().map_or(Selection::None, |s| Selection::Slice(s.id)),
            _ => Selection::None,
        };
        ed.export.open = shot.export_sheet;
        ed.fresh = shot.fresh;
        ed.seek(shot.playhead_ms);
    }
    let mut settings = Settings::default();
    if let Some(dir) = std::env::var_os("GLIDE_OUTPUT_DIR") {
        settings.projects_dir = Some(dir.into());
    }
    let mut devices = Devices::load();
    let mut projects = home::scan_projects(&settings.dir());
    let mut thumbs = Thumbs::default();

    let mut last = None;
    for i in 0..8 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
            time: Some(i as f64 * 0.1),
            system_theme: Some(if shot.dark { egui::Theme::Dark } else { egui::Theme::Light }),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        // Thumbnails load on background threads.
        let wait = match (i, editor.is_some()) {
            (4, true) => 600,
            (4, false) => 5000,
            _ => 20,
        };
        std::thread::sleep(std::time::Duration::from_millis(wait));
        let out = ctx.run_ui(input, |ui| match &mut editor {
            Some(ed) => {
                let mut back = false;
                ed.ui(ui, &mut renderer, &mut back);
            }
            None => {
                home::ui(ui, &mut settings, &mut devices, &mut projects, &mut thumbs, None);
            }
        });
        for (id, deltas) in &out.textures_delta.set {
            for d in deltas {
                renderer.update_texture(&gpu.device, &gpu.queue, *id, d);
            }
        }
        last = Some(out);
    }
    let out = last.unwrap();
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    let (pw, ph) = ((w * ppp) as u32, (h * ppp) as u32);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("snapshot"),
        size: wgpu::Extent3d { width: pw, height: ph, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: [pw, ph], pixels_per_point: out.pixels_per_point };
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    let extra = renderer.update_buffers(&gpu.device, &gpu.queue, &mut enc, &prims, &sd);
    gpu.queue.submit(extra);
    {
        let bg = theme::palette(shot.dark).window;
        let [r, g, b, _] = bg.to_normalized_gamma_f32();
        let mut pass = enc
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("snapshot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        renderer.render(&mut pass, &prims, &sd);
    }
    let rgba = read_texture(&gpu, &target, pw, ph, enc);
    image::save_buffer(out_file, &rgba, pw, ph, image::ExtendedColorType::Rgba8)?;
    Ok(())
}

/// Time the editor preview path (decode + zero-copy upload + render) over
/// consecutive frames, as playback does. Returns frames per second.
pub fn bench_preview(project: &Path, size: (u32, u32), frames: u32) -> Result<f64> {
    let gpu = Gpu::headless()?;
    let mut renderer = egui_wgpu::Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm, egui_wgpu::RendererOptions::default());
    let mut ed = Editor::open(project, gpu.clone())?;
    let preview = &mut ed.preview;
    let step = 1000.0 / 60.0;
    // Warm up (first decode, pipeline creation).
    preview.render(&mut renderer, 0.0, size);
    let t = std::time::Instant::now();
    for i in 1..=frames {
        preview.render(&mut renderer, i as f64 * step, size);
    }
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    Ok(frames as f64 / t.elapsed().as_secs_f64())
}
