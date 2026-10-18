//! Re-encode an oversize export with two-pass rate control, without cutting it short.
use anyhow::{Result, ensure};
use std::{
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
};
pub fn apply(path: &Path, mb: u32, cancel: &AtomicBool) -> Result<()> {
    let limit = mb as u64 * 1_000_000;
    ensure!(limit > 0, "file size target must be positive");
    if std::fs::metadata(path)?.len() <= limit {
        return Ok(());
    }
    let meta = crate::media::ffmpeg::probe(path)?;
    let duration = meta["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    ensure!(duration > 0.0, "cannot determine export duration");
    let audio = meta["streams"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["codec_type"] == "audio");
    let codec = if meta["streams"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["codec_name"] == "hevc")
    {
        "libx265"
    } else {
        "libx264"
    };
    let input = std::fs::canonicalize(path)?;
    let token = format!(
        "{}.{}",
        std::process::id(),
        crate::util::host_now().to_bits()
    );
    let temp = input.with_extension(format!("{token}.fit.mp4"));
    let log = input.with_extension(format!("{token}.fit-pass"));
    let result = (|| -> Result<()> {
        let mut bitrate = (limit as f64 * 8.0 * 0.94 / duration
            - if audio { 128000.0 } else { 0.0 })
        .max(10000.0) as u64;
        for _ in 0..2 {
            for pass in 1..=2 {
                let mut cmd = crate::media::ffmpeg::command("ffmpeg");
                cmd.args(["-v", "error", "-y", "-nostdin", "-i"])
                    .arg(&input)
                    .args([
                        "-map",
                        "0:v:0",
                        "-c:v",
                        codec,
                        "-preset",
                        "fast",
                        "-threads",
                        "2",
                        "-b:v",
                        &bitrate.to_string(),
                    ]);
                if codec == "libx265" {
                    cmd.args([
                        "-x265-params",
                        &format!(
                            "pass={pass}:stats={}:pools=2:log-level=error",
                            log.file_name().unwrap().to_string_lossy()
                        ),
                    ]);
                    cmd.current_dir(input.parent().unwrap());
                } else {
                    cmd.args(["-pass", &pass.to_string(), "-passlogfile"])
                        .arg(&log);
                }
                if pass == 1 {
                    cmd.args(["-an", "-f", "null", "NUL"]);
                } else {
                    if audio {
                        cmd.args(["-map", "0:a:0", "-c:a", "aac", "-b:a", "128k"]);
                    }
                    cmd.arg(&temp);
                }
                let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
                loop {
                    if cancel.load(Ordering::Acquire) {
                        let _ = child.kill();
                        let _ = child.wait();
                        anyhow::bail!("export cancelled");
                    }
                    if let Some(status) = child.try_wait()? {
                        ensure!(status.success(), "size-targeted encoding failed");
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
            let size = std::fs::metadata(&temp)?.len();
            if size <= limit {
                std::fs::rename(&temp, path)?;
                return Ok(());
            }
            bitrate = (bitrate as f64 * limit as f64 / size as f64 * 0.90) as u64;
        }
        anyhow::bail!("cannot fit the complete video into {mb} MB")
    })();
    let _ = std::fs::remove_file(&temp);
    for suffix in ["", "-0.log", "-0.log.mbtree", ".cutree", ".temp"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", log.display()));
    }
    result
}
