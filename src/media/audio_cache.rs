//! Writable application cache with a 2 GiB eviction target. Open assets are
//! leased against deletion; active projects may exceed the target temporarily.
use anyhow::Result;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};
static ROOT: OnceLock<PathBuf> = OnceLock::new();
const BUDGET: u64 = 2 * 1024 * 1024 * 1024;
const USED: &[u8; 8] = b"CAMUSED1";

pub(crate) fn directory() -> Result<PathBuf> {
    if let Some(root) = ROOT.get() {
        return Ok(root.clone());
    }
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Cam/cache/audio-v1"));
    }
    candidates.push(std::env::temp_dir().join("Cam/cache/audio-v1"));
    for candidate in candidates {
        let prepare = || -> Result<PathBuf> {
            fs::create_dir_all(&candidate)?;
            let root = candidate.canonicalize()?;
            drop(super::mux::temporary(&root.join("probe"), "cache")?);
            Ok(root)
        };
        if let Ok(root) = prepare() {
            let _ = ROOT.set(root);
            let root = ROOT.get().unwrap().clone();
            let _ = prune(&root, BUDGET);
            return Ok(root);
        }
    }
    anyhow::bail!(
        "no writable audio cache; check LocalAppData/temp permissions and free disk space"
    )
}
pub(crate) fn owned_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(key) = name
        .strip_prefix("audio-v1-")
        .or_else(|| name.strip_prefix("voice-v3-"))
        .and_then(|s| s.strip_suffix(".pcm"))
    else {
        return false;
    };
    key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit())
}
const LOCK: &[u8; 8] = b"CAMLOCK1";
#[derive(Debug)]
pub(crate) struct Lease {
    file: Option<File>,
    path: PathBuf,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let Some(file) = self.file.take() else { return };
        drop(file);
        // Failed publication owns no data file. Remove only our now-unused
        // lock; existing assets retain a small reusable metadata file.
        if !self.path.is_file() {
            if let Ok(lock) = lock_file(&self.path, true) {
                drop(lock);
                let _ = fs::remove_file(self.path.with_extension("lease"));
            }
        }
    }
}
fn lock_file(path: &Path, exclusive: bool) -> Result<File> {
    let marker = path.with_extension("lease");
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(if exclusive { 0 } else { 3 });
    }
    // Initialize the header while holding an exclusive creation handle so a
    // concurrent opener cannot mistake its partial header for a foreign file.
    let mut create = OpenOptions::new();
    create.read(true).write(true).create_new(true);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        create.share_mode(0);
    }
    match create.open(&marker) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(LOCK) {
                drop(file);
                let _ = fs::remove_file(&marker);
                return Err(error.into());
            }
            if exclusive {
                return Ok(file);
            }
            drop(file);
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    anyhow::ensure!(plain_file(&marker), "cache lock is not a regular file");
    let mut file = options.open(marker)?;
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header)?;
    anyhow::ensure!(
        file.metadata()?.len() == 8 && &header == LOCK,
        "foreign cache lock"
    );
    Ok(file)
}
pub(crate) fn lease(path: &Path) -> Result<Lease> {
    if !owned_name(path) {
        return Ok(Lease {
            file: None,
            path: path.into(),
        });
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match lock_file(path, false) {
            Ok(file) => {
                return Ok(Lease {
                    file: Some(file),
                    path: path.into(),
                });
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| matches!(e.raw_os_error(), Some(2 | 5 | 32 | 33)))
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error),
        }
    }
}
fn plain_file(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}
fn marker_owned(path: &Path) -> bool {
    if !plain_file(path) {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut header = [0u8; 8];
    file.metadata().is_ok_and(|m| m.len() == 8)
        && file.read_exact(&mut header).is_ok()
        && &header == USED
}
fn touch(path: &Path) -> Result<()> {
    let marker = path.with_extension("used");
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => file.write_all(USED)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Never truncate a foreign marker or follow a reparse point.
            if marker_owned(&marker) {
                OpenOptions::new()
                    .write(true)
                    .open(&marker)?
                    .set_times(fs::FileTimes::new().set_modified(SystemTime::now()))?;
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
pub(crate) fn used(path: &Path) {
    if ROOT
        .get()
        .is_some_and(|root| path.parent() == Some(root.as_path()))
        && owned_name(path)
    {
        let _ = touch(path);
        let _ = prune(ROOT.get().unwrap(), BUDGET);
    }
}
/// Delete only recognized complete cache assets directly inside this root.
/// Windows leases reject deletion atomically if any graph still needs a file.
fn prune(root: &Path, budget: u64) -> Result<u64> {
    let root = root.canonicalize()?;
    let mut files = Vec::new();
    let mut total = 0u64;
    for entry in fs::read_dir(&root)? {
        let path = entry?.path();
        if path.parent() != Some(root.as_path()) || !owned_name(&path) || !plain_file(&path) {
            continue;
        }
        let valid = || -> Result<(u64, SystemTime)> {
            let mut file = File::open(&path)?;
            let meta = file.metadata()?;
            let mut header = [0u8; 16];
            file.read_exact(&mut header)?;
            anyhow::ensure!(&header[..8] == b"CAMPCM01", "foreign cache file");
            let frames = u64::from_le_bytes(header[8..].try_into().unwrap());
            anyhow::ensure!(
                frames > 0
                    && frames.checked_mul(8).and_then(|b| b.checked_add(16)) == Some(meta.len()),
                "incomplete cache file"
            );
            let marker = path.with_extension("used");
            let used = if marker_owned(&marker) {
                marker.metadata()?.modified()?
            } else {
                meta.modified()?
            };
            Ok((meta.len(), used))
        };
        if let Ok((bytes, used)) = valid() {
            total = total.saturating_add(bytes);
            files.push((used, path, bytes));
        }
    }
    files.sort_by_key(|file| file.0);
    for (_, path, bytes) in files {
        if total <= budget {
            break;
        }
        let Ok(lock) = lock_file(&path, true) else {
            continue;
        };
        if fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(bytes);
            let marker = path.with_extension("used");
            if marker_owned(&marker) {
                let _ = fs::remove_file(marker);
            }
            drop(lock);
            // A new publisher may have acquired a shared lease after unlock;
            // FILE_SHARE_DELETE protection then makes this cleanup fail safely.
            let _ = fs::remove_file(path.with_extension("lease"));
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::audio_file::AudioFileWriter;
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cam-cache-budget-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        fs::create_dir(&root).unwrap();
        root.canonicalize().unwrap()
    }
    fn asset(root: &Path, key: u64) -> crate::media::audio_file::AudioFile {
        let path = root.join(format!("audio-v1-{key:016x}.pcm"));
        let mut writer = AudioFileWriter::create(&path).unwrap();
        writer.write_frames(&[[0.1; 2]; 1000]).unwrap();
        writer.finish().unwrap()
    }
    #[test]
    fn least_recent_unused_assets_are_evicted_without_touching_foreign_files() {
        let root = root();
        let old = asset(&root, 1).path().to_owned();
        let recent = asset(&root, 2).path().to_owned();
        touch(&old).unwrap();
        touch(&recent).unwrap();
        File::options()
            .write(true)
            .open(old.with_extension("used"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
        let foreign = root.join("audio-v1-0000000000000003.pcm");
        fs::write(&foreign, b"foreign content").unwrap();
        assert_eq!(prune(&root, 8016).unwrap(), 8016);
        assert!(!old.exists() && recent.exists());
        assert_eq!(fs::read(&foreign).unwrap(), b"foreign content");
        fs::write(recent.with_extension("used"), b"foreign marker").unwrap();
        touch(&recent).unwrap();
        assert_eq!(
            fs::read(recent.with_extension("used")).unwrap(),
            b"foreign marker"
        );
    }
    #[test]
    fn live_assets_and_lazy_cursors_keep_their_cache_lease() {
        let root = root();
        let audio = asset(&root, 4);
        let path = audio.path().to_owned();
        let cursor = audio.cursor().unwrap();
        drop(audio);
        assert_eq!(prune(&root, 0).unwrap(), 8016);
        assert!(path.exists());
        drop(cursor);
        assert_eq!(prune(&root, 0).unwrap(), 0);
        assert!(!path.exists());
    }

    #[test]
    fn collection_and_atomic_replacement_coexist_with_a_live_asset() {
        let root = root();
        let live = asset(&root, 9);
        let identity = live.identity().unwrap();
        let worker_root = root.clone();
        let collector = std::thread::spawn(move || {
            for _ in 0..20 {
                assert_eq!(prune(&worker_root, 0).unwrap(), 8016);
            }
        });
        for _ in 0..20 {
            let replaced = asset(&root, 9);
            assert_eq!(replaced.frames(), 1000);
            assert_eq!(replaced.identity().unwrap(), identity);
            assert_eq!(live.identity().unwrap(), identity);
            assert!((replaced.cursor().unwrap().sample_at(500.0).unwrap()[0] - 0.1).abs() < 1e-6);
        }
        collector.join().unwrap();
        drop(live);
        assert_eq!(prune(&root, 0).unwrap(), 0);
    }
}
