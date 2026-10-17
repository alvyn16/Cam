//! Windows capture writer types and sink stub.

pub const TIMESCALE: i32 = 60000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    H264,
    Hevc,
    Prores,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
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

pub struct Sink;

impl Sink {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        _path: Option<&std::path::Path>,
        _v: &VideoSpec,
        _system_audio: bool,
        _mic: bool,
        _layout: AudioLayout,
        _gains_db: [f32; 2],
        _compositor: Option<crate::capture::compose::Compositor>,
        _scene: crate::capture::compose::Scene,
        _preview: Option<PreviewFn>,
    ) -> anyhow::Result<Self> {
        Ok(Sink)
    }

    pub fn emit_until(&mut self, _until: f64) {}
    pub fn lag(&self) -> f64 { 0.05 }
    pub fn t0_secs(&self) -> Option<f64> { Some(0.0) }
    pub fn pause(&mut self, _now: f64) {}
    pub fn resume(&mut self, _now: f64) {}
    pub fn finish(&mut self, _end: f64) -> anyhow::Result<Stats> { Ok(Stats::default()) }
}
