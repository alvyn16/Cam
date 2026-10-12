//! Editor preview: renders the current frame on the UI's GPU device and
//! shows it as an egui texture. Video frames are used in place (zero copy).

use std::path::PathBuf;

use crate::engine::scene::{Frame, Scene};
use crate::media::FrameSource;
use crate::project::model::Project;
use crate::project::recording::Recording;
use crate::render::interop::TextureCache;
use crate::render::{Gpu, OUTPUT_FORMAT, Renderer};

pub struct Preview {
    pub scene: Scene,
    renderer: Renderer,
    cache: Option<TextureCache>,
    screen: Option<FrameSource>,
    camera: Option<FrameSource>,
    camera_offset: f64,
    target: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
    pub texture_id: Option<egui::TextureId>,
    /// (playback frame, height, project revision) last rendered.
    last: Option<(i64, u32, u64)>,
    revision: u64,
    pub last_frame: Option<Frame>,
    package: PathBuf,
}

impl Preview {
    pub fn new(gpu: Gpu, package: PathBuf, project: &Project, rec: &Recording) -> Self {
        let rd = package.join("recording");
        let screen = FrameSource::open(&rd.join(&rec.meta.screen.file)).ok();
        let camera = rec.meta.camera.as_ref().and_then(|c| FrameSource::open(&rd.join(&c.file)).ok());
        let cache = TextureCache::new(&gpu.device);
        let mut renderer = Renderer::new(gpu);
        renderer.prepare(&project.style, rec, Some(&package));
        Preview {
            scene: Scene::new(project.clone(), rec, 60.0),
            renderer,
            cache,
            screen,
            camera,
            camera_offset: rec.meta.camera.as_ref().map_or(0.0, |c| c.offset_ms),
            target: None,
            texture_id: None,
            last: None,
            revision: 0,
            last_frame: None,
            package,
        }
    }

    /// The project changed: rebuild the scene.
    pub fn set_project(&mut self, project: &Project, rec: &Recording) {
        self.scene.set_project(project.clone(), rec);
        self.renderer.prepare(&project.style, rec, Some(&self.package));
        self.revision += 1;
    }

    pub fn output_aspect(&self) -> f32 {
        let o = self.scene.sizes.output;
        (o.x / o.y) as f32
    }

    /// Render the frame at `playback_ms` at `size` pixels (cheap when nothing changed).
    pub fn render(&mut self, egui: &mut egui_wgpu::Renderer, playback_ms: f64, size: (u32, u32)) -> Option<egui::TextureId> {
        if self.draw(playback_ms, size) {
            let device = &self.renderer.gpu().device;
            let view = &self.target.as_ref().unwrap().1;
            match self.texture_id {
                Some(id) => egui.update_egui_texture_from_wgpu_texture(device, view, wgpu::FilterMode::Linear, id),
                None => self.texture_id = Some(egui.register_native_texture(device, view, wgpu::FilterMode::Linear)),
            }
        }
        self.texture_id
    }

    /// Render into the preview texture unless that frame is already there.
    /// Returns whether the texture was (re)created.
    pub fn draw(&mut self, playback_ms: f64, size: (u32, u32)) -> bool {
        let (w, h) = (size.0.max(16), size.1.max(16));
        let f = self.scene.frame_index(playback_ms);
        let key = (f, h, self.revision);
        let gpu = self.renderer.gpu().clone();
        let resized = self.target.as_ref().is_none_or(|t| t.2 != (w, h));
        if !resized && self.last == Some(key) {
            return false;
        }
        if resized {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("preview"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OUTPUT_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.target = Some((texture, view, (w, h)));
        }
        let frame = self.scene.frame(playback_ms, h as f64);
        let src = frame.source_ms;
        if let Some(s) = &mut self.screen
            && let Some(vf) = s.frame_at(src)
        {
            match self.cache.as_ref().and_then(|c| c.wrap(&vf.pb.0, wgpu::TextureUsages::TEXTURE_BINDING)) {
                Some(wr) => self.renderer.set_screen_wrapped(wr),
                None => vf.with_bgra(|v| self.renderer.set_screen(v)),
            }
        }
        if let Some(c) = &mut self.camera
            && let Some(vf) = c.frame_at(src - self.camera_offset)
        {
            match self.cache.as_ref().and_then(|tc| tc.wrap(&vf.pb.0, wgpu::TextureUsages::TEXTURE_BINDING)) {
                Some(wr) => self.renderer.set_camera_wrapped(wr),
                None => vf.with_bgra(|v| self.renderer.set_camera(v)),
            }
        }
        let style = self.scene.project.style.clone();
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let view = &self.target.as_ref().unwrap().1;
        self.renderer.render(&frame, &style, view, &mut enc);
        gpu.queue.submit([enc.finish()]);
        self.last = Some(key);
        self.last_frame = Some(frame);
        resized
    }

    /// The last rendered preview as RGBA (width, height, pixels).
    pub fn read_pixels(&self) -> Option<(u32, u32, Vec<u8>)> {
        let (texture, _, (w, h)) = self.target.as_ref()?;
        let gpu = self.renderer.gpu();
        let enc = gpu.device.create_command_encoder(&Default::default());
        let mut px = crate::render::gpu::read_texture(gpu, texture, *w, *h, enc);
        // The target is BGRA.
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        Some((*w, *h, px))
    }
}
