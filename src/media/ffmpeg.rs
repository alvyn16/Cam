//! Process boundary for the independently decodable media runtime.
use anyhow::{Context, Result, ensure};
use std::{
    path::PathBuf,
    process::{Command, Output},
};

pub fn command(program: &str) -> Command {
    let variable = if program == "ffmpeg" {
        "CAM_FFMPEG"
    } else {
        "CAM_FFPROBE"
    };
    let executable = std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let name = format!("{program}.exe");
            let bundled = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join(&name)));
            bundled
                .filter(|p| p.exists())
                .unwrap_or_else(|| PathBuf::from(name))
        });
    let mut cmd = Command::new(executable);
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    cmd
}

pub fn output(cmd: &mut Command) -> Result<Output> {
    let out = cmd
        .output()
        .context("starting FFmpeg/ffprobe; install FFmpeg or set CAM_FFMPEG and CAM_FFPROBE")?;
    ensure!(
        out.status.success(),
        "media operation failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out)
}

pub fn probe(path: &std::path::Path) -> Result<serde_json::Value> {
    let out = output(
        command("ffprobe")
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
            ])
            .arg(path),
    )?;
    Ok(serde_json::from_slice(&out.stdout)?)
}
