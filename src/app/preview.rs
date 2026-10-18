//! Editor preview: asynchronous, latest-request video decoding; compositing
//! stays on the UI's GPU. Synchronous drawing is retained for offline checks.

use std::path::PathBuf;

use crate::engine::scene::{Frame, Scene};
use crate::media::FrameSource;
use crate::media::preview_decode::{Decoder, Track};
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
    screen_path: PathBuf,
    camera_path: Option<PathBuf>,
    decoder: Decoder,
    pub error: Option<String>,
    synchronous: bool,
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
        let screen_path = rd.join(&rec.meta.screen.file);
        let camera_path = rec.meta.camera.as_ref().map(|c| rd.join(&c.file));
        let track = |t: &crate::project::recording::VideoTrack| Track {
            path: rd.join(&t.file),
            width: t.width,
            height: t.height,
            fps: t.fps,
            duration_ms: (rec.meta.duration_ms - t.offset_ms).max(0.0),
        };
        let decoder = Decoder::new(
            track(&rec.meta.screen),
            rec.meta.camera.as_ref().map(track),
            super::wake,
        );
        let cache = TextureCache::new(&gpu.device);
        let mut renderer = Renderer::new(gpu);
        renderer.prepare(&project.style, rec, Some(&package));
        Preview {
            scene: Scene::new(project.clone(), rec, 60.0),
            renderer,
            cache,
            screen: None,
            camera: None,
            screen_path,
            camera_path,
            decoder,
            error: None,
            synchronous: false,
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
        self.renderer
            .prepare(&project.style, rec, Some(&self.package));
        self.revision += 1;
    }

    pub fn output_aspect(&self) -> f32 {
        let o = self.scene.sizes.output;
        (o.x / o.y) as f32
    }

    /// Render the frame at `playback_ms` at `size` pixels (cheap when nothing changed).
    pub fn render(
        &mut self,
        egui: &mut egui_wgpu::Renderer,
        playback_ms: f64,
        size: (u32, u32),
    ) -> Option<egui::TextureId> {
        if self.draw_mode(playback_ms, size, !self.synchronous) {
            let device = &self.renderer.gpu().device;
            let view = &self.target.as_ref().unwrap().1;
            match self.texture_id {
                Some(id) => egui.update_egui_texture_from_wgpu_texture(
                    device,
                    view,
                    wgpu::FilterMode::Linear,
                    id,
                ),
                None => {
                    self.texture_id =
                        Some(egui.register_native_texture(device, view, wgpu::FilterMode::Linear))
                }
            }
        }
        self.texture_id
    }

    /// Render into the preview texture unless that frame is already there.
    /// Returns whether the texture was (re)created.
    pub fn draw(&mut self, playback_ms: f64, size: (u32, u32)) -> bool {
        self.draw_mode(playback_ms, size, false)
    }

    pub fn pending(&self) -> bool {
        self.decoder.pending()
    }

    /// Offline screenshots/throughput checks must render the requested frame
    /// completely rather than measure asynchronous request submission.
    pub fn set_synchronous(&mut self) {
        self.synchronous = true;
        self.last = None;
    }

    pub fn shutdown(&mut self) {
        self.decoder.shutdown();
    }

    fn draw_mode(&mut self, playback_ms: f64, size: (u32, u32), background: bool) -> bool {
        let trace = super::scrub_trace() && playback_ms > 0.0;
        if trace {
            super::scrub_log(format_args!(
                "scrub-preview: request background={background}"
            ));
        }
        let (w, h) = (size.0.max(16), size.1.max(16));
        let f = self.scene.frame_index(playback_ms);
        let key = (f, h, self.revision);
        let gpu = self.renderer.gpu().clone();
        let resized = self.target.as_ref().is_none_or(|t| t.2 != (w, h));
        let src = self.scene.map.to_source(playback_ms);
        if background {
            self.decoder.request(
                src,
                self.camera_path
                    .as_ref()
                    .map(|_| (src - self.camera_offset).max(0.0)),
            );
            if let Some(decoded) = self.decoder.take_ready() {
                if trace {
                    super::scrub_log(format_args!("scrub-preview: upload"));
                }
                if let Some(screen) = decoded.screen {
                    screen.with_bgra(|v| self.renderer.set_screen(v));
                }
                if let Some(camera) = decoded.camera {
                    camera.with_bgra(|v| self.renderer.set_camera(v));
                }
                self.error = decoded.error;
                self.last = None;
            }
        }
        if !resized && self.last == Some(key) {
            return false;
        }
        if trace {
            super::scrub_log(format_args!("scrub-preview: scene"));
        }
        if resized {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("preview"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OUTPUT_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.target = Some((texture, view, (w, h)));
        }
        let frame = self.scene.frame(playback_ms, h as f64);
        if trace {
            super::scrub_log(format_args!("scrub-preview: scene-done"));
        }
        let src = frame.source_ms;
        if !background {
            if self.screen.is_none() {
                self.screen = FrameSource::open(&self.screen_path).ok();
            }
            if self.camera.is_none() {
                self.camera = self
                    .camera_path
                    .as_ref()
                    .and_then(|p| FrameSource::open(p).ok());
            }
            if let Some(s) = &mut self.screen
                && let Some(vf) = s.frame_at(src)
            {
                match vf.try_wrap(self.cache.as_ref()) {
                    Some(wr) => self.renderer.set_screen_wrapped(wr),
                    None => vf.with_bgra(|v| self.renderer.set_screen(v)),
                }
            }
            if let Some(c) = &mut self.camera
                && let Some(vf) = c.frame_at(src - self.camera_offset)
            {
                match vf.try_wrap(self.cache.as_ref()) {
                    Some(wr) => self.renderer.set_camera_wrapped(wr),
                    None => vf.with_bgra(|v| self.renderer.set_camera(v)),
                }
            }
        }
        let style = self.scene.project.style.clone();
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let view = &self.target.as_ref().unwrap().1;
        if trace {
            super::scrub_log(format_args!("scrub-preview: render"));
        }
        self.renderer.render(&frame, &style, view, &mut enc);
        if trace {
            super::scrub_log(format_args!("scrub-preview: submit"));
        }
        gpu.queue.submit([enc.finish()]);
        if trace {
            super::scrub_log(format_args!("scrub-preview: done"));
        }
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
