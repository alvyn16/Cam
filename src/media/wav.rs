//! Seekable 48 kHz stereo float WAV/RF64 output with constant memory use.
use anyhow::{Result, ensure};
use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::Path,
};

const HEADER: u64 = 80;
const FRAME_BYTES: u64 = 8;
pub(crate) struct WaveWriter(File);
impl WaveWriter {
    pub fn create(path: &Path) -> Result<Self> {
        let mut file = File::options().write(true).create_new(true).open(path)?;
        if let Err(error) = file.write_all(&header(0)) {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(error.into());
        }
        Ok(Self(file))
    }
    pub fn write_at(&mut self, frame: u64, stereo: &[f32]) -> Result<()> {
        ensure!(stereo.len() % 2 == 0, "incomplete stereo audio frame");
        let offset = frame
            .checked_mul(FRAME_BYTES)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or_else(|| anyhow::anyhow!("audio timestamp exceeds file limits"))?;
        self.0.seek(SeekFrom::Start(offset))?;
        // Windows targets are little-endian, matching IEEE float WAV.
        self.0.write_all(bytemuck::cast_slice(stereo))?;
        Ok(())
    }
    pub fn finish(mut self, frames: u64) -> Result<()> {
        let bytes = frames
            .checked_mul(FRAME_BYTES)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or_else(|| anyhow::anyhow!("audio duration exceeds file limits"))?;
        self.0.set_len(bytes)?;
        self.0.rewind()?;
        self.0.write_all(&header(frames))?;
        self.0.sync_all()?;
        Ok(())
    }
}
fn header(frames: u64) -> Vec<u8> {
    let data = frames * FRAME_BYTES;
    let riff = data + HEADER - 8;
    let rf64 = riff > u32::MAX as u64;
    let mut h = Vec::with_capacity(HEADER as usize);
    h.extend_from_slice(if rf64 { b"RF64" } else { b"RIFF" });
    h.extend_from_slice(&(if rf64 { u32::MAX } else { riff as u32 }).to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(if rf64 { b"ds64" } else { b"JUNK" });
    h.extend_from_slice(&28u32.to_le_bytes());
    h.extend_from_slice(&riff.to_le_bytes());
    h.extend_from_slice(&data.to_le_bytes());
    h.extend_from_slice(&frames.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&3u16.to_le_bytes()); // WAVE_FORMAT_IEEE_FLOAT
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&48000u32.to_le_bytes());
    h.extend_from_slice(&384000u32.to_le_bytes());
    h.extend_from_slice(&8u16.to_le_bytes());
    h.extend_from_slice(&32u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(if rf64 { u32::MAX } else { data as u32 }).to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seekable_audio_preserves_timestamps_silence_and_exact_duration() {
        let path = std::env::temp_dir().join(format!(
            "cam-wave-{}-{}.wav",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        let mut w = WaveWriter::create(&path).unwrap();
        w.write_at(100, &[0.25, -0.5, 0.75, -1.0]).unwrap();
        w.write_at(600, &[0.9, 0.9]).unwrap();
        w.finish(480).unwrap(); // truncate samples beyond the stopped video
        let probe = crate::media::ffmpeg::probe(&path).unwrap();
        assert_eq!(probe["streams"][0]["codec_name"], "pcm_f32le");
        let audio = crate::media::read_audio(&path, 0).unwrap();
        assert_eq!(audio.len(), 480);
        assert!(audio[..100].iter().all(|s| *s == [0.0; 2]));
        assert_eq!(audio[100], [0.25, -0.5]);
        assert_eq!(audio[101], [0.75, -1.0]);
        assert!(audio[102..].iter().all(|s| *s == [0.0; 2]));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn rf64_header_keeps_large_duration_without_32_bit_wraparound() {
        let frames = 48_000 * 4 * 3600;
        let h = header(frames);
        assert_eq!(h.len(), HEADER as usize);
        assert_eq!(&h[..4], b"RF64");
        assert_eq!(&h[12..16], b"ds64");
        assert_eq!(
            u64::from_le_bytes(h[28..36].try_into().unwrap()),
            frames * FRAME_BYTES
        );
        assert_eq!(u64::from_le_bytes(h[36..44].try_into().unwrap()), frames);
    }
}
