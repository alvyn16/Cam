//! Immutable decoded audio on disk; cursors never allocate by track duration.
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Stdio},
};

pub type Stereo = [f32; 2];
const MAGIC: &[u8; 8] = b"CAMPCM01";
const HEADER: u64 = 16;
const FRAME_BYTES: u64 = 8;
pub const WINDOW_FRAMES: usize = 4096;
fn staging_path(destination: &Path, kind: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    destination.with_extension(format!(
        "{}.{}.{}.{kind}",
        std::process::id(),
        crate::util::host_now().to_bits(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[derive(Clone, Debug)]
pub struct AudioFile {
    path: PathBuf,
    frames: u64,
    lease: std::sync::Arc<super::audio_cache::Lease>,
}
impl AudioFile {
    pub fn decode_cached(
        source: &Path,
        track: usize,
        cache: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self> {
        ensure!(!cancelled(), "audio decoding cancelled");
        let source = source
            .canonicalize()
            .with_context(|| format!("opening audio {}", source.display()))?;
        let key = fingerprint(&source, track)?;
        std::fs::create_dir_all(cache)
            .with_context(|| format!("creating audio cache {}", cache.display()))?;
        let cache = cache.canonicalize()?;
        let destination = cache.join(format!("audio-v1-{key:016x}.pcm"));
        if let Ok(asset) = Self::open_pcm(&destination) {
            return Ok(asset);
        }
        let (pending, mut output) = PendingFile::create(staging_path(&destination, "pending"))?;
        output.write_all(MAGIC)?;
        output.write_all(&0u64.to_le_bytes())?;
        let (log_file, log) = PendingFile::create(staging_path(&destination, "log"))?;
        let mut decoder = Decoder(
            super::ffmpeg::command("ffmpeg")
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(&source)
                .args([
                    "-map",
                    &format!("0:a:{track}"),
                    "-vn",
                    "-ac",
                    "2",
                    "-ar",
                    "48000",
                    "-f",
                    "f32le",
                    "pipe:1",
                ])
                .stdout(Stdio::from(output))
                .stderr(Stdio::from(log))
                .spawn()
                .with_context(|| format!("starting audio decoder for {}", source.display()))?,
        );
        let status = loop {
            ensure!(!cancelled(), "audio decoding cancelled");
            if let Some(status) = decoder.0.try_wait()? {
                break status;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        if !status.success() {
            let mut log = Vec::new();
            File::open(&log_file.path)?
                .take(64 * 1024)
                .read_to_end(&mut log)?;
            anyhow::bail!(
                "decoding audio {} (track {track}): {}",
                source.display(),
                String::from_utf8_lossy(&log).trim()
            );
        }
        ensure!(
            fingerprint(&source, track)? == key,
            "audio source changed during decoding: {}",
            source.display()
        );
        ensure!(!cancelled(), "audio decoding cancelled");
        let mut file = File::options().read(true).write(true).open(&pending.path)?;
        let len = file.metadata()?.len();
        ensure!(
            len > HEADER && (len - HEADER) % FRAME_BYTES == 0,
            "decoder produced incomplete or empty audio: {}",
            source.display()
        );
        file.seek(SeekFrom::Start(8))?;
        file.write_all(&((len - HEADER) / FRAME_BYTES).to_le_bytes())?;
        file.sync_all()?;
        drop(file);
        let _publication = super::audio_cache::lease(&destination)?;
        match std::fs::rename(&pending.path, &destination) {
            Ok(()) => Self::open_pcm(&destination),
            Err(e) => Self::open_pcm(&destination).with_context(|| {
                format!("publishing decoded audio {}: {e}", destination.display())
            }),
        }
    }
    pub(crate) fn open_pcm(path: &Path) -> Result<Self> {
        ensure!(
            path.is_file(),
            "decoded audio file is missing: {}",
            path.display()
        );
        let lease = super::audio_cache::lease(path)?;
        let mut file = File::open(path)?;
        let mut header = [0; HEADER as usize];
        file.read_exact(&mut header)?;
        ensure!(&header[..8] == MAGIC, "invalid decoded-audio cache header");
        let frames = u64::from_le_bytes(header[8..].try_into().unwrap());
        let bytes = frames
            .checked_mul(FRAME_BYTES)
            .and_then(|n| n.checked_add(HEADER));
        ensure!(
            frames > 0 && bytes == Some(file.metadata()?.len()),
            "incomplete decoded-audio cache"
        );
        let asset = Self {
            path: path.into(),
            frames,
            lease: std::sync::Arc::new(lease),
        };
        super::audio_cache::used(path);
        Ok(asset)
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub(crate) fn identity(&self) -> Result<u64> {
        let path = self.path.canonicalize()?;
        if super::audio_cache::owned_name(&path) {
            // Cache names identify immutable source content. Concurrent
            // publication must not invalidate enhancement solely because the
            // winning file has a different write time.
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut hash);
            self.frames.hash(&mut hash);
            "immutable-pcm-v1".hash(&mut hash);
            return Ok(hash.finish());
        }
        fingerprint(&path, 0)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn cursor(&self) -> Result<AudioCursor> {
        Ok(AudioCursor {
            file: File::open(&self.path)
                .with_context(|| format!("opening decoded audio {}", self.path.display()))?,
            frames: self.frames,
            base: 0,
            window: Vec::new(),
            _lease: self.lease.clone(),
        })
    }
    pub fn peak(&self, cancelled: &dyn Fn() -> bool) -> Result<f32> {
        let mut cursor = self.cursor()?;
        let mut chunk = vec![[0.0; 2]; WINDOW_FRAMES];
        let (mut at, mut peak) = (0, 0.0f32);
        while at < self.frames {
            ensure!(!cancelled(), "audio peak scan cancelled");
            let count = cursor.read_frames(at, &mut chunk)?;
            for s in &chunk[..count] {
                peak = peak.max(s[0].abs()).max(s[1].abs());
            }
            at += count as u64;
        }
        Ok(peak)
    }
}
/// Transactional, chunked producer for derived audio assets (DSP/mixes).
pub struct AudioFileWriter {
    file: Option<File>,
    pending: PendingFile,
    destination: PathBuf,
    frames: u64,
}
impl AudioFileWriter {
    pub fn create(destination: &Path) -> Result<Self> {
        if let Some(parent) = destination.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let (pending, mut file) = PendingFile::create(staging_path(destination, "pending"))?;
        file.write_all(MAGIC)?;
        file.write_all(&0u64.to_le_bytes())?;
        Ok(Self {
            file: Some(file),
            pending,
            destination: destination.into(),
            frames: 0,
        })
    }
    pub fn write_frames(&mut self, samples: &[Stereo]) -> Result<()> {
        let frames = self
            .frames
            .checked_add(samples.len() as u64)
            .context("audio frame count overflow")?;
        ensure!(
            frames
                .checked_mul(FRAME_BYTES)
                .and_then(|n| n.checked_add(HEADER))
                .is_some(),
            "audio file size overflow"
        );
        self.file
            .as_mut()
            .context("audio writer already closed")?
            .write_all(bytemuck::cast_slice(samples))?;
        self.frames = frames;
        Ok(())
    }
    pub fn finish(mut self) -> Result<AudioFile> {
        ensure!(self.frames > 0, "cannot publish an empty audio asset");
        let mut file = self.file.take().context("audio writer already closed")?;
        file.seek(SeekFrom::Start(8))?;
        file.write_all(&self.frames.to_le_bytes())?;
        file.sync_all()?;
        drop(file);
        let _publication = super::audio_cache::lease(&self.destination)?;
        std::fs::rename(&self.pending.path, &self.destination)
            .with_context(|| format!("publishing audio {}", self.destination.display()))?;
        AudioFile::open_pcm(&self.destination)
    }
}
fn fingerprint(source: &Path, track: usize) -> Result<u64> {
    let metadata = source
        .metadata()
        .with_context(|| format!("reading audio source {}", source.display()))?;
    ensure!(
        metadata.is_file(),
        "audio source is not a regular file: {}",
        source.display()
    );
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hash);
    metadata.len().hash(&mut hash);
    metadata.modified()?.hash(&mut hash);
    track.hash(&mut hash);
    "f32le:48000:stereo:v1".hash(&mut hash);
    Ok(hash.finish())
}
struct PendingFile {
    path: PathBuf,
}
impl PendingFile {
    fn create(path: PathBuf) -> Result<(Self, File)> {
        // Only claim cleanup ownership after create_new succeeds.
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok((Self { path }, file))
    }
}
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
struct Decoder(Child);
impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub struct AudioCursor {
    file: File,
    frames: u64,
    base: u64,
    window: Vec<Stereo>,
    _lease: std::sync::Arc<super::audio_cache::Lease>,
}
impl AudioCursor {
    pub fn read_frames(&mut self, start: u64, output: &mut [Stereo]) -> Result<usize> {
        output.fill([0.0; 2]);
        let count = self.frames.saturating_sub(start).min(output.len() as u64) as usize;
        if count > 0 {
            self.file
                .seek(SeekFrom::Start(HEADER + start * FRAME_BYTES))?;
            self.file
                .read_exact(bytemuck::cast_slice_mut(&mut output[..count]))?;
        }
        Ok(count)
    }
    fn frame(&mut self, index: u64) -> Result<Stereo> {
        if index >= self.frames {
            return Ok([0.0; 2]);
        }
        if index < self.base || index >= self.base + self.window.len() as u64 {
            self.base = index / WINDOW_FRAMES as u64 * WINDOW_FRAMES as u64;
            let count = (self.frames - self.base).min(WINDOW_FRAMES as u64 + 1) as usize;
            self.window.resize(count, [0.0; 2]);
            let result = self
                .file
                .seek(SeekFrom::Start(HEADER + self.base * FRAME_BYTES))
                .and_then(|_| {
                    self.file
                        .read_exact(bytemuck::cast_slice_mut(&mut self.window))
                });
            if let Err(error) = result {
                self.window.clear();
                return Err(error).context("reading decoded audio window");
            }
        }
        Ok(self.window[(index - self.base) as usize])
    }
    /// Same zero-padding and final-frame behavior as export::audio::sample.
    pub fn sample_at(&mut self, position: f64) -> Result<Stereo> {
        if !position.is_finite() || position < 0.0 || position >= self.frames as f64 {
            return Ok([0.0; 2]);
        }
        let index = position as u64;
        let a = self.frame(index)?;
        if index + 1 >= self.frames {
            return Ok(a);
        }
        let b = self.frame(index + 1)?;
        let f = (position - index as f64) as f32;
        Ok([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cam-audio-assets-{label}-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn failed_window_read_cannot_be_reused_as_successful_zero_samples() {
        let dir = fixture("read-error");
        let path = dir.join("source.pcm");
        let mut writer = AudioFileWriter::create(&path).unwrap();
        writer.write_frames(&vec![[0.25; 2]; 10000]).unwrap();
        let asset = writer.finish().unwrap();
        let mut cursor = asset.cursor().unwrap();
        assert_eq!(cursor.sample_at(0.0).unwrap(), [0.25; 2]);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(HEADER + FRAME_BYTES)
            .unwrap();
        assert!(cursor.sample_at(9000.0).is_err());
        assert!(cursor.sample_at(9000.0).is_err());
        assert!(cursor.window.is_empty());
    }
    fn tone(path: &Path, hz: u32, rate: u32, duration: f64) {
        super::super::ffmpeg::output(
            super::super::ffmpeg::command("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency={hz}:sample_rate={rate}:duration={duration}"),
                    "-y",
                ])
                .arg(path),
        )
        .unwrap();
    }
    #[test]
    fn cached_decode_matches_independent_ffmpeg_for_resampled_compressed_audio() {
        let dir = fixture("decode");
        for (name, rate) in [("mono.wav", 22050), ("mono.mp3", 44100)] {
            let source = dir.join(name);
            tone(&source, 731, rate, 0.4);
            let asset =
                AudioFile::decode_cached(&source, 0, &dir.join("cache"), &|| false).unwrap();
            let expected = crate::media::read_audio(&source, 0).unwrap();
            assert_eq!(asset.frames(), expected.len() as u64);
            let mut cursor = asset.cursor().unwrap();
            for position in [0.0, 11.25, 4095.5, 8191.75, expected.len() as f64 - 1.0] {
                assert_eq!(
                    cursor.sample_at(position).unwrap(),
                    crate::export::audio::sample(&expected, position)
                );
            }
            let cached =
                AudioFile::decode_cached(&source, 0, &dir.join("cache"), &|| false).unwrap();
            assert_eq!(cached.path(), asset.path());
            assert!(
                (asset.peak(&|| false).unwrap()
                    - expected
                        .iter()
                        .fold(0.0f32, |p, s| p.max(s[0].abs()).max(s[1].abs())))
                .abs()
                    < 1e-6
            );
        }
        assert!(std::fs::read_dir(dir.join("cache")).unwrap().all(|e| {
            e.unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "pcm" || e == "lease")
        }));
    }
    #[test]
    fn source_changes_invalidate_cache_and_invalid_tracks_leave_no_partial_assets() {
        let dir = fixture("invalidate");
        let source = dir.join("tone.wav");
        let cache = dir.join("cache");
        tone(&source, 440, 48000, 0.2);
        let old = AudioFile::decode_cached(&source, 0, &cache, &|| false).unwrap();
        tone(&source, 880, 48000, 0.3);
        let new = AudioFile::decode_cached(&source, 0, &cache, &|| false).unwrap();
        assert_ne!(new.path(), old.path());
        assert!(new.frames() > old.frames());
        assert_ne!(
            new.cursor().unwrap().sample_at(100.0).unwrap(),
            old.cursor().unwrap().sample_at(100.0).unwrap()
        );
        assert!(AudioFile::decode_cached(&source, 1, &cache, &|| false).is_err());
        let before = std::fs::read_dir(&cache).unwrap().count();
        assert!(AudioFile::decode_cached(&source, 0, &cache, &|| true).is_err());
        assert_eq!(before, std::fs::read_dir(&cache).unwrap().count());
        assert_eq!(
            std::fs::read_dir(&cache)
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|e| e == "pcm"))
                .count(),
            2
        );
    }
    #[test]
    fn cancellation_after_spawn_removes_only_its_staging_files() {
        let dir = fixture("cancel");
        let source = dir.join("tone.wav");
        let cache = dir.join("cache");
        tone(&source, 440, 48000, 0.4);
        let polls = std::cell::Cell::new(0);
        let result = AudioFile::decode_cached(&source, 0, &cache, &|| {
            polls.set(polls.get() + 1);
            polls.get() >= 2
        });
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(polls.get() >= 2);
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        assert!(source.exists());
    }
    #[test]
    fn concurrent_publication_is_complete_and_truncated_cache_is_rebuilt() {
        let dir = fixture("concurrent");
        let source = dir.join("tone.wav");
        let cache = dir.join("cache");
        tone(&source, 550, 48000, 0.3);
        let jobs: Vec<_> = (0..3)
            .map(|_| {
                let (source, cache) = (source.clone(), cache.clone());
                std::thread::spawn(move || {
                    AudioFile::decode_cached(&source, 0, &cache, &|| false).unwrap()
                })
            })
            .collect();
        let assets: Vec<_> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
        assert!(
            assets
                .iter()
                .all(|a| a.path() == assets[0].path() && a.frames() == 14400)
        );
        assert_eq!(
            std::fs::read_dir(&cache)
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|e| e == "pcm"))
                .count(),
            1
        );
        File::options()
            .write(true)
            .open(assets[0].path())
            .unwrap()
            .set_len(HEADER + 8)
            .unwrap();
        let repaired = AudioFile::decode_cached(&source, 0, &cache, &|| false).unwrap();
        assert_eq!(repaired.frames(), 14400);
        assert!(repaired.cursor().unwrap().sample_at(500.0).unwrap()[0].abs() > 0.01);
    }
    #[test]
    fn selected_audio_streams_have_distinct_samples_and_cache_entries() {
        let dir = fixture("streams");
        let source = dir.join("tracks.mka");
        super::super::ffmpeg::output(
            super::super::ffmpeg::command("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000:duration=0.3",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=880:sample_rate=48000:duration=0.3",
                    "-map",
                    "0:a",
                    "-map",
                    "1:a",
                    "-c:a",
                    "pcm_f32le",
                    "-y",
                ])
                .arg(&source),
        )
        .unwrap();
        let a = AudioFile::decode_cached(&source, 0, &dir.join("cache"), &|| false).unwrap();
        let b = AudioFile::decode_cached(&source, 1, &dir.join("cache"), &|| false).unwrap();
        assert_ne!(a.path(), b.path());
        for (asset, index) in [(&a, 0), (&b, 1)] {
            let samples = crate::media::read_audio(&source, index).unwrap();
            assert_eq!(
                asset.cursor().unwrap().sample_at(100.5).unwrap(),
                crate::export::audio::sample(&samples, 100.5)
            );
        }
        assert_ne!(
            a.cursor().unwrap().sample_at(100.5).unwrap(),
            b.cursor().unwrap().sample_at(100.5).unwrap()
        );
    }
    #[test]
    fn peak_scan_finds_late_samples_without_full_track_allocation() {
        let dir = fixture("peak");
        let path = dir.join("large.pcm");
        let frames = 4_800_000u64;
        let mut file = File::create(&path).unwrap();
        file.write_all(MAGIC).unwrap();
        file.write_all(&frames.to_le_bytes()).unwrap();
        file.set_len(HEADER + frames * FRAME_BYTES).unwrap();
        file.seek(SeekFrom::Start(HEADER + (frames - 1) * FRAME_BYTES))
            .unwrap();
        file.write_all(bytemuck::cast_slice(&[0.75f32, -0.9]))
            .unwrap();
        drop(file);
        let asset = AudioFile::open_pcm(&path).unwrap();
        assert_eq!(asset.peak(&|| false).unwrap(), 0.9);
        assert_eq!(
            asset
                .cursor()
                .unwrap()
                .sample_at(frames as f64 - 1.0)
                .unwrap(),
            [0.75, -0.9]
        );
    }
    #[test]
    fn staged_writer_publishes_complete_audio_and_cancellation_preserves_destination() {
        let dir = fixture("writer");
        let destination = dir.join("mix.pcm");
        std::fs::write(&destination, b"existing cache").unwrap();
        let mut writer = AudioFileWriter::create(&destination).unwrap();
        let pending = writer.pending.path.clone();
        writer.write_frames(&[[0.2, -0.2]; 20]).unwrap();
        drop(writer);
        assert!(!pending.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing cache");
        let mut writer = AudioFileWriter::create(&destination).unwrap();
        writer.write_frames(&[[0.2, -0.2]; 20]).unwrap();
        writer.write_frames(&[[0.4, -0.4]; 10]).unwrap();
        let asset = writer.finish().unwrap();
        assert_eq!(asset.frames(), 30);
        assert_eq!(
            asset.cursor().unwrap().sample_at(19.5).unwrap(),
            [0.3, -0.3]
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
    #[test]
    fn staging_collision_never_deletes_a_preexisting_file() {
        let dir = fixture("collision");
        let path = dir.join("existing.pending");
        std::fs::write(&path, b"keep").unwrap();
        assert!(PendingFile::create(path.clone()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"keep");
    }
    #[test]
    fn changed_source_and_failed_publication_do_not_publish_partial_audio() {
        let dir = fixture("publication");
        let source = dir.join("tone.wav");
        let cache = dir.join("cache");
        tone(&source, 440, 48000, 0.3);
        let handle = File::options().write(true).open(&source).unwrap();
        let polls = std::cell::Cell::new(0);
        let result = AudioFile::decode_cached(&source, 0, &cache, &|| {
            polls.set(polls.get() + 1);
            if polls.get() == 2 {
                handle
                    .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3))
                    .unwrap();
            }
            false
        });
        assert!(result.unwrap_err().to_string().contains("source changed"));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        drop(handle);
        let key = fingerprint(&source.canonicalize().unwrap(), 0).unwrap();
        let blocked = cache.join(format!("audio-v1-{key:016x}.pcm"));
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("keep"), b"keep").unwrap();
        assert!(AudioFile::decode_cached(&source, 0, &cache, &|| false).is_err());
        assert_eq!(std::fs::read(blocked.join("keep")).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
    }
    #[test]
    fn cursor_matches_reference_across_windows_seeks_and_track_end() {
        let path = std::env::temp_dir().join(format!(
            "cam-audio-cursor-{}-{}.pcm",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        let samples: Vec<Stereo> = (0..10_000)
            .map(|i| [(i as f32 * 0.01).sin(), (i as f32 * 0.02).cos()])
            .collect();
        let mut file = File::create(&path).unwrap();
        file.write_all(MAGIC).unwrap();
        file.write_all(&(samples.len() as u64).to_le_bytes())
            .unwrap();
        file.write_all(bytemuck::cast_slice(&samples)).unwrap();
        drop(file);
        let asset = AudioFile::open_pcm(&path).unwrap();
        let mut cursor = asset.cursor().unwrap();
        for position in [
            -1.0, 0.0, 123.25, 4095.75, 8191.5, 4096.0, 5.5, 9999.75, 10_000.0,
        ] {
            assert_eq!(
                cursor.sample_at(position).unwrap(),
                crate::export::audio::sample(&samples, position)
            );
            assert!(cursor.window.capacity() <= (WINDOW_FRAMES + 1) * 2);
        }
        let mut tail = [[9.0; 2]; 8];
        assert_eq!(cursor.read_frames(9998, &mut tail).unwrap(), 2);
        assert_eq!(&tail[..2], &samples[9998..]);
        assert!(tail[2..].iter().all(|s| *s == [0.0; 2]));
        assert!((asset.peak(&|| false).unwrap() - 1.0).abs() < 1e-6);
        assert!(asset.peak(&|| true).is_err());
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
}
