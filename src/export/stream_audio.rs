//! A complete soundtrack rendered to an owned WAV/RF64 file in bounded blocks.
use super::block_mix::{BLOCK_FRAMES, BlockMixer};
use crate::{
    engine::timemap::TimeMap,
    media::wav::WaveWriter,
    project::{model::Project, recording::Recording},
};
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};

pub struct Soundtrack {
    path: PathBuf,
}
impl Soundtrack {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for Soundtrack {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
pub fn render(
    project: &Project,
    recording: &Recording,
    dir: Option<&Path>,
    cache: &Path,
    map: &TimeMap,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Soundtrack>> {
    let Some(mixer) = BlockMixer::new(project, recording, dir, cache, map, cancelled)? else {
        return Ok(None);
    };
    ensure!(mixer.frames() > 0, "empty audio timeline");
    std::fs::create_dir_all(cache)?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = cache.join(format!(
        "mix-output-{}-{stamp}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let writer = WaveWriter::create(&path)?;
    // This guard is created only after create_new succeeds. The helper owns
    // and closes the writer before errors reach the guard's cleanup.
    let owned = Soundtrack { path };
    write(mixer, writer, cancelled)?;
    ensure!(!cancelled(), "audio rendering cancelled");
    Ok(Some(owned))
}
fn write(
    mut mixer: BlockMixer,
    mut writer: WaveWriter,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let mut block = vec![[0.0; 2]; BLOCK_FRAMES];
    let mut at = 0;
    while at < mixer.frames() {
        let count = (mixer.frames() - at).min(BLOCK_FRAMES as u64) as usize;
        mixer.render(at, &mut block[..count], true, cancelled)?;
        writer.write_at(at, bytemuck::cast_slice(&block[..count]))?;
        at += count as u64;
    }
    ensure!(!cancelled(), "audio rendering cancelled");
    writer.finish(mixer.frames())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::recording::{AudioTrack, RecordingMeta};
    fn fixture() -> (PathBuf, Project, Recording, TimeMap) {
        let dir = std::env::temp_dir().join(format!(
            "cam-stream-soundtrack-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir_all(dir.join("recording")).unwrap();
        let samples: Vec<_> = (0..62400)
            .map(|i| [(i as f32 * 0.06).sin() * 0.1; 2])
            .collect();
        crate::app::player::write_wav(&dir.join("recording/system.wav"), &samples).unwrap();
        let mut project = Project::default();
        project.style.audio.volume = 0.5;
        let recording = Recording {
            meta: RecordingMeta {
                duration_ms: 1300.0,
                system_audio: Some(AudioTrack {
                    file: "system.wav".into(),
                    track: 0,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let map = TimeMap::new(&[], 1300.0);
        (dir, project, recording, map)
    }
    #[test]
    fn wave_matches_reference_and_is_removed_when_ownership_ends() {
        let (dir, project, recording, map) = fixture();
        let track = render(
            &project,
            &recording,
            Some(&dir),
            &dir.join("cache"),
            &map,
            &|| false,
        )
        .unwrap()
        .unwrap();
        let expected = super::super::audio::mix(&project, &recording, Some(&dir), &map).unwrap();
        let actual = crate::media::read_audio(track.path(), 0).unwrap();
        assert_eq!(actual, expected);
        let path = track.path().to_path_buf();
        drop(track);
        assert!(!path.exists());
        assert!(dir.join("recording/system.wav").exists());
    }
    #[test]
    fn cancellation_and_silent_projects_leave_no_temporary_soundtrack() {
        let (dir, mut project, recording, map) = fixture();
        let cache = dir.join("cache");
        for after_block in [false, true] {
            let cancelled = || {
                cache.read_dir().is_ok_and(|mut entries| {
                    entries.any(|e| {
                        let e = e.unwrap();
                        if !e.file_name().to_string_lossy().starts_with("mix-output-") {
                            return false;
                        }
                        !after_block
                            || std::fs::File::open(e.path())
                                .unwrap()
                                .metadata()
                                .unwrap()
                                .len()
                                > 80
                    })
                })
            };
            assert!(render(&project, &recording, Some(&dir), &cache, &map, &cancelled).is_err());
        }
        assert!(cache.read_dir().unwrap().all(|e| {
            e.unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "pcm" || e == "lease")
        }));
        project.style.audio.mute_system = true;
        assert!(
            render(&project, &recording, Some(&dir), &cache, &map, &|| false)
                .unwrap()
                .is_none()
        );
    }
}
