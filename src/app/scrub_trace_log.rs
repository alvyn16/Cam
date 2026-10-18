//! Optional diagnostics never write files or pipes on the UI thread.
use std::{
    fmt,
    fs::File,
    io::{BufWriter, Write},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};
struct Trace {
    sender: mpsc::SyncSender<String>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
static TRACE: OnceLock<Option<Trace>> = OnceLock::new();
pub(super) fn enabled() -> bool {
    TRACE
        .get_or_init(|| {
            std::env::var_os("CAM_SCRUB_TRACE")?;
            let path = std::env::var_os("CAM_SCRUB_TRACE_FILE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::temp_dir().join(format!("cam-scrub-{}.trace", std::process::id()))
                });
            let file = File::create(path).ok()?;
            let (sender, receiver) = mpsc::sync_channel::<String>(512);
            let stop = Arc::new(AtomicBool::new(false));
            let end = stop.clone();
            let worker = std::thread::spawn(move || {
                let mut out = BufWriter::with_capacity(64 * 1024, file);
                loop {
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(line) => {
                            let _ = writeln!(out, "{line}");
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = out.flush();
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    if end.load(Ordering::Acquire) {
                        for line in receiver.try_iter() {
                            let _ = writeln!(out, "{line}");
                        }
                        break;
                    }
                }
                let _ = out.flush();
            });
            Some(Trace {
                sender,
                stop,
                worker: Mutex::new(Some(worker)),
            })
        })
        .is_some()
}
pub(super) fn log(message: fmt::Arguments<'_>) {
    if let Some(Some(trace)) = TRACE.get() {
        let _ = trace.sender.try_send(message.to_string());
    }
}
pub(super) fn shutdown() {
    if let Some(Some(trace)) = TRACE.get() {
        trace.stop.store(true, Ordering::Release);
        if let Some(worker) = trace.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
