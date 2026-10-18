//! Transactional muxing of borrowed encoded video and optional disk audio.
use super::ffmpeg;
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Stdio},
};

pub(crate) struct Pending(PathBuf);
impl Pending {
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn create(output: &Path, kind: &str) -> Result<(Pending, File)> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let extension = output.extension().and_then(|s| s.to_str()).unwrap_or("mp4");
    let path = output.with_extension(format!(
        "{}.{stamp}.{}.{kind}.{extension}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let file = File::options().write(true).create_new(true).open(&path)?;
    Ok((Pending(path), file))
}

pub(crate) fn temporary(output: &Path, kind: &str) -> Result<Pending> {
    let (owned, file) = create(output, kind)?;
    drop(file);
    Ok(owned)
}

/// Inputs are borrowed and never deleted. Only exclusively created output/log
/// staging files belong to this operation. Cancellation kills and reaps FFmpeg.
pub(crate) fn publish(
    video: &Path,
    audio: Option<&Path>,
    output: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    ensure!(!cancelled(), "video/audio mux cancelled");
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let (complete, file) = create(output, "complete")?;
    drop(file);
    let (log, stderr) = create(output, "mux-log")?;
    let mut command = ffmpeg::command("ffmpeg");
    command
        .args(["-v", "error", "-nostdin", "-y", "-i"])
        .arg(video);
    if let Some(audio) = audio {
        command.arg("-i").arg(audio).args([
            "-map", "0:v:0", "-map", "1:a:0", "-c:a", "aac", "-b:a", "192k",
        ]);
    } else {
        command.args(["-map", "0:v:0", "-an"]);
    }
    command
        .args(["-c:v", "copy", "-movflags", "+faststart"])
        .arg(&complete.0);
    let mut process = Process(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("starting video/audio muxer")?,
    );
    let status = loop {
        ensure!(!cancelled(), "video/audio mux cancelled");
        if let Some(status) = process.0.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    if !status.success() {
        let mut error = Vec::new();
        File::open(&log.0)?
            .take(64 * 1024)
            .read_to_end(&mut error)?;
        anyhow::bail!(
            "video/audio mux failed: {}",
            String::from_utf8_lossy(&error).trim()
        );
    }
    ensure!(!cancelled(), "video/audio mux cancelled");
    ensure!(
        complete.0.metadata()?.len() > 0,
        "video/audio mux produced an empty file"
    );
    std::fs::rename(&complete.0, output).context("publishing muxed video")?;
    Ok(())
}
