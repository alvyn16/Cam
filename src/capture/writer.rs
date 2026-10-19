//! Camera capture sink with a pause-adjusted constant frame rate.

pub const TIMESCALE: i32 = 60000;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    H264,
    Hevc,
    Prores,
}
impl From<Codec> for crate::project::model::Codec {
    fn from(value: Codec) -> Self {
        match value {
            Codec::H264 => Self::H264,
            Codec::Hevc => Self::Hevc,
            Codec::Prores => Self::Prores,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VideoSpec {
    pub width: usize,
    pub height: usize,
    pub fps: u32,
    pub codec: Codec,
    pub bitrate: u64,
    pub quality: Option<f64>,
    pub pixel_format: u32,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum AudioLayout {
    Both,
    Mix,
    Stems,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioKind {
    System,
    Mic,
}

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub screen_frames_in: u64,
    pub cam_frames_in: u64,
    pub video_frames_out: u64,
    pub video_dups: u64,
    pub video_drops: u64,
    pub audio_sys_buffers: u64,
    pub audio_mic_buffers: u64,
    pub audio_drops: u64,
    pub audio_mix_buffers: u64,
    pub audio_clipped_samples: u64,
    pub paused_slots: u64,
    pub screen_latency_max: f64,
    pub screen_latency_sum: f64,
    pub cam_latency_max: f64,
    pub cam_latency_sum: f64,
}

pub type PreviewFn = std::sync::Arc<dyn Fn(&[u8], u32, u32) + Send + Sync>;

pub struct Sink {
    writer: Option<crate::media::writer::MovieWriter>,
    latest: Option<crate::media::PixelBuffer>,
    clock: super::audio::Clock,
    fps: u32,
    error: Option<String>,
}
impl Sink {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: Option<&std::path::Path>,
        v: &VideoSpec,
        _system_audio: bool,
        _mic: bool,
        _layout: AudioLayout,
        _gains_db: [f32; 2],
        _compositor: Option<crate::capture::compose::Compositor>,
        _scene: crate::capture::compose::Scene,
        _preview: Option<PreviewFn>,
    ) -> anyhow::Result<Self> {
        let path = path.ok_or_else(|| anyhow::anyhow!("camera output path required"))?;
        let spec = crate::media::writer::WriterSpec {
            width: v.width as u32,
            height: v.height as u32,
            fps: v.fps,
            codec: v.codec.into(),
            bitrate: v.bitrate,
            quality: v.quality,
            audio: false,
        };
        Ok(Self {
            writer: Some(crate::media::writer::MovieWriter::create(path, &spec)?),
            latest: None,
            clock: Default::default(),
            fps: v.fps,
            error: None,
        })
    }
    pub fn push_video(&mut self, f: &crate::capture::grab::Image) {
        if self.clock.pause.is_some() {
            return;
        }
        self.clock.start.get_or_insert_with(crate::util::host_now);
        if let Some(w) = &mut self.writer {
            let result = (|| -> anyhow::Result<()> {
                let pb = w.buffer()?;
                let data = if (w.width, w.height) == (f.width, f.height) {
                    f.bgra.clone()
                } else {
                    let image = image::RgbaImage::from_raw(f.width, f.height, f.bgra.clone())
                        .ok_or_else(|| anyhow::anyhow!("invalid camera pixels"))?;
                    image::imageops::resize(
                        &image,
                        w.width,
                        w.height,
                        image::imageops::FilterType::Triangle,
                    )
                    .into_raw()
                };
                *pb.bgra.lock().unwrap() = data;
                self.latest = Some(pb);
                Ok(())
            })();
            if let Err(e) = result {
                self.error = Some(format!("{e:#}"));
            }
        }
    }
    pub fn emit_until(&mut self, until: f64) {
        let wanted = (self.clock.elapsed(until) * self.fps as f64).ceil() as u64;
        if let (Some(writer), Some(frame)) = (&mut self.writer, &self.latest) {
            while writer.frames < wanted {
                if let Err(e) = writer.push_frame(frame) {
                    self.error = Some(format!("{e:#}"));
                    break;
                }
            }
        }
    }
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }
    pub fn lag(&self) -> f64 {
        0.05
    }
    pub fn t0_secs(&self) -> Option<f64> {
        self.clock.start
    }
    pub fn pause(&mut self, now: f64) {
        self.emit_until(now);
        self.clock.pause(now);
    }
    pub fn resume(&mut self, now: f64) {
        self.clock.resume(now);
    }
    pub fn finish(&mut self, end: f64) -> anyhow::Result<Stats> {
        self.emit_until(end);
        if let Some(e) = self.error.take() {
            anyhow::bail!(e);
        }
        if let Some(w) = self.writer.take() {
            let n = w.frames;
            w.finish()?;
            return Ok(Stats {
                video_frames_out: n,
                ..Default::default()
            });
        }
        Ok(Stats::default())
    }
}
