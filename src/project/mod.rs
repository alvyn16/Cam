//! `.cam` packages: a folder holding the editable project and the
//! untouched recording.
//!
//! ```text
//! Name.cam/
//!   project.json        style, timeline, export settings
//!   recording/
//!     meta.json         what was recorded
//!     screen.mov        screen video (no cursor) + audio tracks
//!     camera.mov        camera video, if recorded
//!     mouse.json        mouse events
//!     keys.json         key presses
//!     cursors.json      cursor images (PNG files in cursors/)
//!     transcript.json   captions, once transcribed
//! ```

pub mod model;
pub mod presets;
pub mod recording;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::engine::zoom;
use model::{DeviceFrame, Project, Slice, Style};
use recording::{Recording, RecordingMeta, SourceKind};

pub const EXTENSION: &str = "cam";

#[derive(Clone, Debug)]
pub struct Package {
    pub root: PathBuf,
}

impl Package {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Package { root: root.into() }
    }
    pub fn project_file(&self) -> PathBuf {
        self.root.join("project.json")
    }
    pub fn recording_dir(&self) -> PathBuf {
        self.root.join("recording")
    }
    pub fn rec(&self, name: &str) -> PathBuf {
        self.recording_dir().join(name)
    }
    pub fn name(&self) -> String {
        self.root
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn load_recording(&self) -> Result<Recording> {
        let meta: RecordingMeta = read_json(&self.rec("meta.json"))?;
        Ok(Recording {
            meta,
            mouse: read_json_or_default(&self.rec("mouse.json"))?,
            keys: read_json_or_default(&self.rec("keys.json"))?,
            cursors: read_json_or_default(&self.rec("cursors.json"))?,
            transcript: read_json_or_default(&self.rec("transcript.json"))?,
        })
    }

    pub fn load_project(&self) -> Result<Project> {
        read_json(&self.project_file())
    }

    pub fn load(&self) -> Result<(Project, Recording)> {
        let rec = self.load_recording()?;
        let project = match self.load_project() {
            Ok(p) => p,
            Err(_) => new_project(&self.name(), &rec),
        };
        Ok((project, rec))
    }

    /// Copy a file into the package's `assets/` folder; returns the path
    /// relative to the package, so projects stay self-contained.
    pub fn import_asset(&self, src: &Path, stem: &str) -> Result<String> {
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_ascii_lowercase();
        let dir = self.root.join("assets");
        std::fs::create_dir_all(&dir)?;
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let name = format!("{stem}-{ms}.{ext}");
        std::fs::copy(src, dir.join(&name))
            .with_context(|| format!("copying {}", src.display()))?;
        Ok(format!("assets/{name}"))
    }

    pub fn save_project(&self, p: &Project) -> Result<()> {
        write_json(&self.project_file(), p)
    }
}

/// A fresh project for a recording: one slice covering everything,
/// automatic zooms on clicks, and a style suited to the source.
pub fn new_project(name: &str, rec: &Recording) -> Project {
    let mut p = Project {
        name: name.into(),
        ..Default::default()
    };
    p.id = format!(
        "{:016x}",
        rec.meta.created_ms as u64 ^ 0x9e37_79b9_7f4a_7c15
    );
    p.created_ms = rec.meta.created_ms;
    p.updated_ms = rec.meta.created_ms;
    if rec.meta.kind == SourceKind::Display {
        // Full-screen recordings sit on the background too (a little less
        // padding than windows), so the background settings show from the
        // start; set Padding to 0 % for an edge-to-edge video.
        p.style.padding = Style::REVEAL_PADDING;
    }
    if rec.meta.kind == SourceKind::Device {
        // iPhone/iPad screens get a device frame; there's no cursor to follow.
        let (w, h) = (rec.meta.screen.width, rec.meta.screen.height);
        p.style.device = if h > w {
            DeviceFrame::Phone
        } else {
            DeviceFrame::Tablet
        };
        p.style.padding = 8.0;
    }
    let id = p.timeline.new_id();
    p.timeline.slices.push(Slice {
        id,
        start: 0.0,
        end: rec.meta.duration_ms,
        ..Default::default()
    });
    let events = crate::engine::events::Events::new(rec.mouse.clone());
    let zooms = zoom::auto_zooms(
        &events.downs,
        rec.meta.duration_ms,
        p.style.default_zoom,
        &mut p.timeline,
    );
    p.timeline.zooms = zooms;
    p
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))
}

fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    if path.exists() {
        read_json(path)
    } else {
        Ok(T::default())
    }
}

/// Write JSON atomically (temp file + rename) so a crash never leaves a
/// half-written project.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_json_cancellable(path, value, &|| false)
}

pub(crate) fn write_json_cancellable<T: Serialize>(
    path: &Path,
    value: &T,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    anyhow::ensure!(!cancelled(), "saving JSON cancelled");
    let pending = crate::media::mux::temporary(&path.with_extension("json.tmp"), "json")?;
    let mut file = std::io::BufWriter::with_capacity(
        65536,
        fs::OpenOptions::new().write(true).open(pending.path())?,
    );
    serde_json::to_writer_pretty(&mut file, value)
        .with_context(|| format!("writing {}", path.display()))?;
    std::io::Write::flush(&mut file)?;
    file.get_ref().sync_all()?;
    drop(file);
    anyhow::ensure!(!cancelled(), "saving JSON cancelled");
    fs::rename(pending.path(), path).with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod atomic_json_tests {
    use super::*;
    #[test]
    fn cancelled_and_failed_writes_preserve_destination_and_foreign_temporary_file() {
        let dir = std::env::temp_dir().join(format!(
            "cam-atomic-json-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("transcript.json");
        let foreign = path.with_extension("json.tmp");
        fs::write(&path, b"old transcript").unwrap();
        fs::write(&foreign, b"foreign temporary file").unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        assert!(
            write_json_cancellable(&path, &vec!["new"], &|| calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                > 0)
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"old transcript");
        assert_eq!(fs::read(&foreign).unwrap(), b"foreign temporary file");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        write_json(&path, &vec!["new"]).unwrap();
        assert_eq!(read_json::<Vec<String>>(&path).unwrap(), ["new"]);
        assert_eq!(fs::read(&foreign).unwrap(), b"foreign temporary file");
        let blocked = dir.join("blocked.json");
        fs::create_dir(&blocked).unwrap();
        assert!(write_json(&blocked, &vec!["new"]).is_err());
        assert!(blocked.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
    }
}

/// Default folder for new projects.
pub fn projects_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(user_profile) = std::env::var_os("USERPROFILE") {
            let p = PathBuf::from(user_profile).join("Videos").join("Cam");
            return p;
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Videos").join("Cam")
}

/// A new, unused package path like `~/Movies/Glide/Recording 2026-10-11 at 11.30.12.cam`.
pub fn new_package_path(dir: &Path, stamp: &str) -> PathBuf {
    let base = format!("Recording {stamp}");
    let mut path = dir.join(format!("{base}.{EXTENSION}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} {n}.{EXTENSION}"));
        n += 1;
    }
    path
}
