//! Probe the driver, not just FFmpeg's compiled-in list of encoders.
use crate::project::model::Codec;
use std::sync::OnceLock;
pub fn hardware(codec: Codec, width: u32, height: u32) -> Option<&'static str> {
    if width < 256 || height < 128 || std::env::var("CAM_ENCODER").as_deref() == Ok("software") {
        return None;
    }
    static H264: OnceLock<Option<&str>> = OnceLock::new();
    static HEVC: OnceLock<Option<&str>> = OnceLock::new();
    let (cache, candidates) = match codec {
        Codec::H264 => (&H264, ["h264_nvenc", "h264_qsv", "h264_amf"]),
        Codec::Hevc => (&HEVC, ["hevc_nvenc", "hevc_qsv", "hevc_amf"]),
        Codec::Prores => return None,
    };
    *cache.get_or_init(|| {
        candidates.into_iter().find(|name| {
            super::ffmpeg::command("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=size=256x256:rate=30",
                    "-frames:v",
                    "1",
                    "-c:v",
                    name,
                    "-f",
                    "null",
                    "-",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    })
}
