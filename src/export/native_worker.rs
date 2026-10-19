//! One native encoder lifetime per child process. Driver/MF handles cannot
//! accumulate in the editor; a kill-on-close job owns the complete child tree.
use super::{
    Job, ProgressFn, Stats,
    native::{self, Policy},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use windows62::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::*,
};

const WORKER: &str = "__native-export-worker";
#[derive(Serialize, Deserialize)]
struct Request {
    version: u32,
    job: Job,
    required: bool,
}
#[derive(Serialize, Deserialize)]
enum Outcome {
    Complete(Stats),
    Unavailable,
    Failed(String),
}

struct Directory(PathBuf);
impl Directory {
    fn create(output: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = parent.join(format!(
            ".cam-native-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Process {
    child: Child,
    job: OwnedHandle,
}
impl Drop for Process {
    fn drop(&mut self) {
        // Includes FFmpeg descendants, even after the native worker crashes.
        unsafe {
            let _ = TerminateJobObject(self.job.0, 1);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn executable() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    // Cargo's test harness is not the application. Hardware tests use the
    // binary built in the same profile, never a separately installed version.
    if exe.parent().is_some_and(|p| p.ends_with("deps")) {
        let app = exe.parent().unwrap().parent().unwrap().join("cam.exe");
        ensure!(
            app.is_file(),
            "build cam.exe before running native-worker hardware tests"
        );
        return Ok(app);
    }
    Ok(exe)
}
fn cleanup_job() -> Result<OwnedHandle> {
    let job = OwnedHandle(unsafe { CreateJobObjectW(None, None)? });
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        )?;
    }
    Ok(job)
}
fn spawn(directory: &Path) -> Result<Process> {
    let job = cleanup_job()?;
    let log = File::create(directory.join("worker.log"))?;
    let child = Command::new(executable()?)
        .arg(WORKER)
        .arg(directory)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .context("starting native export worker")?;
    let mut process = Process { child, job };
    unsafe {
        AssignProcessToJobObject(process.job.0, HANDLE(process.child.as_raw_handle()))
            .context("assigning native worker to its cleanup job")?;
    }
    // The child cannot start media work before assignment succeeds.
    process
        .child
        .stdin
        .take()
        .context("native worker handshake missing")?
        .write_all(&[1])?;
    eprintln!("Cam native export worker: {}", process.child.id());
    Ok(process)
}
fn read_log(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(size.saturating_sub(64 * 1024)));
    let mut bytes = Vec::new();
    let _ = file.take(64 * 1024).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}
fn progress_at(path: &Path) -> Option<f64> {
    let mut bytes = [0u8; 8];
    File::open(path).ok()?.read_exact(&mut bytes).ok()?;
    let value = f64::from_le_bytes(bytes);
    (value.is_finite() && (0.0..=1.0).contains(&value)).then_some(value)
}
pub(super) fn try_export(
    job: &Job,
    progress: ProgressFn,
    cancel: &AtomicBool,
    policy: Policy,
) -> Result<Option<Stats>> {
    let started = Instant::now();
    use crate::project::model::{Codec, ExportFormat};
    if policy == Policy::Off || job.settings.format != ExportFormat::Mp4 {
        return Ok(None);
    }
    let scene = crate::engine::scene::Scene::new(
        job.project.clone(),
        &job.recording,
        job.settings.fps as f64,
    );
    let (w, h) = scene.sizes.render_size(job.settings.height);
    if job.settings.codec_for(w, h) == Codec::Prores {
        ensure!(
            policy != Policy::Require,
            "native ProRes export is unavailable"
        );
        return Ok(None);
    }
    ensure!(!cancel.load(Ordering::Acquire), "export cancelled");
    let directory = Directory::create(&job.output)?;
    let mut request_job = job.clone();
    request_job.output = directory.0.join("complete.mp4");
    serde_json::to_writer(
        File::create(directory.0.join("request.json"))?,
        &Request {
            version: 1,
            job: request_job,
            required: policy == Policy::Require,
        },
    )?;
    let mut process = match spawn(&directory.0) {
        Ok(process) => process,
        Err(error) if policy == Policy::Auto => {
            eprintln!("Cam native worker unavailable; using FFmpeg: {error:#}");
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let mut cancelled_at = None;
    let mut last = -1.0;
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            if cancelled_at.is_none() {
                std::fs::write(directory.0.join("cancel"), [])?;
                cancelled_at = Some(Instant::now());
            }
            ensure!(
                cancelled_at.unwrap().elapsed() < Duration::from_secs(3),
                "native export cancelled"
            );
        }
        if let Some(value) = progress_at(&directory.0.join("progress")) {
            if value > last {
                progress(value);
                last = value;
            }
        }
        if let Some(status) = process.child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Kill/reap any surviving descendants before touching files or publishing.
    drop(process);
    let log = read_log(&directory.0.join("worker.log"));
    if !log.is_empty() {
        eprint!("{log}");
    }
    ensure!(!cancel.load(Ordering::Acquire), "native export cancelled");
    ensure!(
        status.success(),
        "native export worker failed ({status}): {log}"
    );
    let mut bytes = Vec::new();
    File::open(directory.0.join("result.json"))
        .context("native worker returned no result")?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "native worker result is too large"
    );
    match serde_json::from_slice::<Outcome>(&bytes)? {
        Outcome::Complete(mut stats) => {
            std::fs::rename(directory.0.join("complete.mp4"), &job.output)
                .context("publishing native worker output")?;
            stats.seconds = started.elapsed().as_secs_f64();
            Ok(Some(stats))
        }
        Outcome::Unavailable => Ok(None),
        Outcome::Failed(error) => anyhow::bail!("{error}"),
    }
}

/// Private process entry point, handled before parsing the public CLI.
pub fn entry() -> Result<bool> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(WORKER)) {
        return Ok(false);
    }
    let directory = PathBuf::from(args.next().context("native worker directory missing")?);
    ensure!(args.next().is_none(), "unexpected native worker argument");
    let mut handshake = [0u8; 1];
    std::io::stdin().read_exact(&mut handshake)?;
    ensure!(handshake == [1], "invalid native worker handshake");
    let request: Request = serde_json::from_reader(File::open(directory.join("request.json"))?)?;
    ensure!(request.version == 1, "unsupported native worker protocol");
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let (worker_cancel, worker_done, cancel_path) =
        (cancel.clone(), done.clone(), directory.join("cancel"));
    let mut progress_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join("progress"))?;
    let watcher = std::thread::spawn(move || {
        while !worker_done.load(Ordering::Acquire) {
            if cancel_path.exists() {
                worker_cancel.store(true, Ordering::Release);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let job = request.job;
    let mut scene = crate::engine::scene::Scene::new(
        job.project.clone(),
        &job.recording,
        job.settings.fps as f64,
    );
    let result = native::try_export_in(
        &job,
        &mut scene,
        &mut |value| {
            let _ = progress_file.seek(SeekFrom::Start(0));
            let _ = progress_file.write_all(&value.to_le_bytes());
        },
        &cancel,
        if request.required {
            Policy::Require
        } else {
            Policy::Auto
        },
        Some(&directory),
    );
    done.store(true, Ordering::Release);
    let _ = watcher.join();
    let outcome = match result {
        Ok(Some(stats)) => Outcome::Complete(stats),
        Ok(None) => Outcome::Unavailable,
        Err(error) => Outcome::Failed(format!("{error:#}").chars().take(65536).collect()),
    };
    serde_json::to_writer(File::create(directory.join("result.json"))?, &outcome)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "checks forced cleanup of an actual worker and FFmpeg descendant"]
    fn forced_cleanup_terminates_descendants() {
        use windows62::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::*};
        let dir = Directory::create(&std::env::temp_dir().join("job-cleanup.mp4")).unwrap();
        // Use a native fixture: terminating the PowerShell CLR can itself
        // stall on this PC, independently of whether the job killed FFmpeg.
        let script = dir.0.join("child.cmd");
        std::fs::write(&script, "@echo off\r\nset /p cam_handshake=\r\n\"%CAM_CLEANUP_CHILD%\" -v error -re -f lavfi -i anullsrc=r=48000:cl=stereo -t 60 -f null NUL\r\n").unwrap();
        let job = cleanup_job().unwrap();
        let command = crate::media::ffmpeg::command("ffmpeg");
        let child = Command::new("cmd.exe")
            .args(["/d", "/s", "/c"])
            .raw_arg(format!("\"\"{}\"\"", script.display()))
            .env("CAM_CLEANUP_CHILD", command.get_program())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let mut process = Process { child, job };
        unsafe {
            AssignProcessToJobObject(process.job.0, HANDLE(process.child.as_raw_handle())).unwrap();
        }
        process
            .child
            .stdin
            .take()
            .unwrap()
            .write_all(b"start\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        #[repr(C)]
        struct ProcessIds { assigned: u32, count: u32, ids: [usize; 16] }
        let pid = loop {
            let mut list = ProcessIds { assigned: 0, count: 0, ids: [0; 16] };
            unsafe { QueryInformationJobObject(Some(process.job.0), JobObjectBasicProcessIdList, (&mut list as *mut ProcessIds).cast(), std::mem::size_of_val(&list) as u32, None).unwrap(); }
            if let Some(pid) = list.ids[..list.count as usize].iter().copied().find(|&pid| pid != process.child.id() as usize) {
                break pid as u32;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not start its descendant"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let descendant = OwnedHandle(unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
            .unwrap()
        });
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.0, 0) },
            windows62::Win32::Foundation::WAIT_TIMEOUT
        );
        drop(process);
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.0, 2000) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn progress_rejects_truncated_nonfinite_and_out_of_range_values() {
        let dir = Directory::create(&std::env::temp_dir().join("progress.mp4")).unwrap();
        let path = dir.0.join("progress");
        for bytes in [
            vec![],
            vec![1; 4],
            f64::NAN.to_le_bytes().to_vec(),
            f64::INFINITY.to_le_bytes().to_vec(),
            (-0.1f64).to_le_bytes().to_vec(),
            1.1f64.to_le_bytes().to_vec(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(progress_at(&path), None);
        }
        std::fs::write(&path, 0.75f64.to_le_bytes()).unwrap();
        assert_eq!(progress_at(&path), Some(0.75));
    }

    #[test]
    #[ignore = "requires the built application and connected native hardware encoder"]
    fn cancelled_and_failed_workers_preserve_output_and_clean_scratch() {
        let dir = Directory::create(&std::env::temp_dir().join("worker-test.mp4")).unwrap();
        let output = dir.0.join("existing.mp4");
        std::fs::write(&output, b"previous export").unwrap();
        let mut job = Job {
            project: Default::default(),
            recording: crate::synth::recording(320, 200, 5000.0),
            source: super::super::Source::Synthetic,
            settings: crate::project::model::ExportSettings {
                height: 240,
                fps: 20,
                ..Default::default()
            },
            output: output.clone(),
        };
        let cancel = AtomicBool::new(false);
        let mut updates = 0;
        let started = Instant::now();
        let result = try_export(
            &job,
            &mut |value| {
                updates += 1;
                if value >= 0.1 {
                    cancel.store(true, Ordering::Release);
                }
            },
            &cancel,
            Policy::Require,
        );
        assert!(result.is_err());
        assert!(cancel.load(Ordering::Acquire) && updates > 0);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(std::fs::read(&output).unwrap(), b"previous export");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);

        job.source = super::super::Source::Package(dir.0.join("missing.cam"));
        let result = try_export(&job, &mut |_| {}, &AtomicBool::new(false), Policy::Require);
        assert!(result.is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"previous export");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);

        // Reaching an unavailable encoder must preserve the automatic fallback.
        job.source = super::super::Source::Synthetic;
        job.settings.codec = crate::project::model::Codec::Prores;
        assert!(
            try_export(&job, &mut |_| {}, &AtomicBool::new(false), Policy::Auto)
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"previous export");
    }
}
