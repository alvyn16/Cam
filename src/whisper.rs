//! Captions with Whisper (whisper.cpp): on-device, no Speech
//! permission needed, word-level timings.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use anyhow::{Context, Result, bail};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::project::recording::Word;

/// Whisper model sizes offered for captions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    Base,
    Small,
    Medium,
}

impl Model {
    pub const ALL: [Model; 3] = [Model::Base, Model::Small, Model::Medium];

    pub fn name(self) -> &'static str {
        match self {
            Model::Base => "base",
            Model::Small => "small",
            Model::Medium => "medium",
        }
    }

    pub fn parse(s: &str) -> Option<Model> {
        Model::ALL.into_iter().find(|m| m.name() == s)
    }

    /// Approximate download size, MB.
    pub fn size_mb(self) -> u32 {
        match self {
            Model::Base => 148,
            Model::Small => 488,
            Model::Medium => 1533,
        }
    }

    fn file(self) -> String {
        format!("ggml-{}.bin", self.name())
    }

    fn url(self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.file()
        )
    }

    pub fn path(self) -> PathBuf {
        models_dir().join(self.file())
    }

    pub fn is_downloaded(self) -> bool {
        std::fs::metadata(self.path()).is_ok_and(|m| m.len() > (self.size_mb() as u64) * 900_000)
    }
}

fn models_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("Cam").join("models");
    }
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("AppData/Roaming/Cam/models")
}

/// The largest model already on disk.
pub fn best_downloaded() -> Option<Model> {
    Model::ALL.into_iter().rev().find(|m| m.is_downloaded())
}

/// Progress shared with the UI, in tenths of a percent (0..=1000), with the
/// current phase.
#[derive(Default)]
pub struct Progress {
    pub permille: AtomicU32,
    /// 0 = downloading, 1 = transcribing.
    pub phase: AtomicU32,
    cancelled: AtomicBool,
}
impl Progress {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub(crate) fn check(&self) -> Result<()> {
        anyhow::ensure!(!self.is_cancelled(), "transcription cancelled");
        Ok(())
    }
}

struct Download(std::process::Child);
impl Drop for Download {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Download `model` unless it's already there (with curl, into a
/// partial file that is renamed when complete).
pub fn download(model: Model, progress: &Progress) -> Result<PathBuf> {
    progress.check()?;
    let path = model.path();
    if model.is_downloaded() {
        return Ok(path);
    }
    std::fs::create_dir_all(models_dir())?;
    let part = crate::media::mux::temporary(&path, "download")?;
    let log = crate::media::mux::temporary(&path.with_extension("log"), "download-log")?;
    let curl_bin = if cfg!(target_os = "windows") {
        "curl.exe"
    } else {
        "curl"
    };
    let mut command = std::process::Command::new(curl_bin);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Download(
        command
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--retry",
                "3",
                "--output",
            ])
            .arg(part.path())
            .arg(model.url())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(log.path())?)
            .spawn()
            .context("starting the model download")?,
    );
    let expected = model.size_mb() as f64 * 1_000_000.0;
    let status = loop {
        progress.check()?;
        if let Some(s) = child.0.try_wait()? {
            break s;
        }
        let got = std::fs::metadata(part.path()).map_or(0, |m| m.len()) as f64;
        progress.permille.store(
            ((got / expected).min(0.99) * 1000.0) as u32,
            Ordering::Relaxed,
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    if !status.success() {
        use std::io::Read;
        let mut err = Vec::new();
        std::fs::File::open(log.path())?
            .take(65536)
            .read_to_end(&mut err)?;
        bail!(
            "couldn't download the Whisper {} model: {}",
            model.name(),
            String::from_utf8_lossy(&err).trim()
        );
    }
    anyhow::ensure!(
        std::fs::metadata(part.path())?.len() > model.size_mb() as u64 * 900000,
        "the downloaded Whisper {} model is incomplete",
        model.name()
    );
    progress.check()?;
    std::fs::rename(part.path(), &path)?;
    Ok(path)
}

/// Is this whisper token text a bracketed annotation rather than speech
/// (e.g. "[BLANK_AUDIO]", "(music)")?
fn is_annotation(t: &str) -> bool {
    let t = t.trim();
    t.is_empty()
        || (t.starts_with('[') && t.ends_with(']'))
        || (t.starts_with('(') && t.ends_with(')'))
        || t.starts_with("[_")
}

/// Merge whisper's per-word segments: a segment that doesn't start with a
/// space continues the previous word (e.g. "don" + "'t").
pub fn merge_words(segments: &[(f64, f64, String)]) -> Vec<Word> {
    let mut out: Vec<Word> = vec![];
    for (start, end, text) in segments {
        if is_annotation(text) {
            continue;
        }
        match out.last_mut() {
            Some(w) if !text.starts_with(' ') => {
                w.text.push_str(text.trim());
                w.end = *end;
            }
            _ => out.push(Word {
                start: *start,
                end: *end,
                text: text.trim().to_string(),
            }),
        }
    }
    out.retain(|w| !w.text.is_empty());
    out
}

/// The state retains its model context and is reused across bounded chunks.
pub(crate) struct Session {
    state: whisper_rs::WhisperState,
    progress: Arc<Progress>,
}
pub(crate) fn has_signal(samples: &[f32]) -> bool {
    samples.iter().any(|sample| sample.abs() >= 1e-4)
}
fn speech_range(samples: &[f32]) -> Option<std::ops::Range<usize>> {
    let first = samples.iter().position(|s| s.abs() >= 1e-4)?;
    let last = samples.iter().rposition(|s| s.abs() >= 1e-4)?;
    Some(first.saturating_sub(8000)..(last + 1 + 8000).min(samples.len()))
}
struct CallProgress<'a> {
    progress: &'a Progress,
    offset_ms: f64,
    duration_ms: f64,
    total_ms: f64,
}
unsafe extern "C" fn report_progress(
    _: *mut whisper_rs::WhisperSysContext,
    _: *mut whisper_rs::WhisperSysState,
    percent: i32,
    data: *mut std::ffi::c_void,
) {
    if data.is_null() {
        return;
    }
    let call = unsafe { &*data.cast::<CallProgress<'_>>() };
    let fraction =
        (call.offset_ms + call.duration_ms * percent.clamp(0, 100) as f64 / 100.0) / call.total_ms;
    call.progress.permille.store(
        (fraction.clamp(0.0, 1.0) * 1000.0) as u32,
        Ordering::Relaxed,
    );
}
unsafe extern "C" fn should_abort(data: *mut std::ffi::c_void) -> bool {
    data.is_null()
        || unsafe { &*data.cast::<CallProgress<'_>>() }
            .progress
            .is_cancelled()
}
impl Session {
    pub(crate) fn new(model: Model, progress: &Arc<Progress>) -> Result<Self> {
        progress.check()?;
        progress.phase.store(0, Ordering::Relaxed);
        let path = download(model, progress)?;
        progress.check()?;
        progress.phase.store(1, Ordering::Relaxed);
        whisper_rs::install_logging_hooks();
        let ctx = WhisperContext::new_with_params(&path, WhisperContextParameters::default())
            .map_err(|e| anyhow::anyhow!("loading the Whisper model: {e}"))?;
        progress.check()?;
        let state = ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("Whisper: {e}"))?;
        progress.check()?;
        Ok(Self {
            state,
            progress: progress.clone(),
        })
    }
    pub(crate) fn chunk(
        &mut self,
        samples: &[f32],
        offset_ms: f64,
        total_ms: f64,
    ) -> Result<Vec<Word>> {
        self.progress.check()?;
        anyhow::ensure!(
            samples.iter().all(|s| s.is_finite()),
            "non-finite transcription audio"
        );
        anyhow::ensure!(
            samples.len() <= 880000,
            "transcription chunk exceeds 55 seconds"
        );
        anyhow::ensure!(
            offset_ms.is_finite() && offset_ms >= 0.0 && total_ms.is_finite() && total_ms > 0.0,
            "invalid transcription timing"
        );
        let Some(range) = speech_range(samples) else {
            return Ok(Vec::new());
        };
        let offset_ms = offset_ms + range.start as f64 / 16.0;
        let samples = &samples[range];
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32;
        params.set_n_threads(threads);
        params.set_language(Some("auto"));
        params.set_translate(false);
        params.set_no_context(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_token_timestamps(true);
        params.set_split_on_word(true);
        params.set_max_len(1);
        let call = CallProgress {
            progress: &self.progress,
            offset_ms,
            duration_ms: samples.len() as f64 / 16.0,
            total_ms,
        };
        let user_data = (&call as *const CallProgress<'_>).cast_mut().cast();
        // SAFETY: whisper_full_with_state is synchronous. Both callbacks only
        // access this call's live context/atomics, never retain the pointer, and
        // cannot unwind. No heap-allocated callback closure outlives the call.
        unsafe {
            params.set_progress_callback(Some(report_progress));
            params.set_progress_callback_user_data(user_data);
            params.set_abort_callback(Some(should_abort));
            params.set_abort_callback_user_data(user_data);
        }
        let result = self.state.full(params, samples);
        self.progress.check()?;
        result.map_err(|e| anyhow::anyhow!("Whisper transcription failed: {e}"))?;
        let mut segments = Vec::new();
        for seg in self.state.as_iter() {
            let text = seg
                .to_str_lossy()
                .map(|t| t.into_owned())
                .unwrap_or_default();
            segments.push((
                seg.start_timestamp() as f64 * 10.0,
                seg.end_timestamp() as f64 * 10.0,
                text,
            ));
        }
        let mut words = merge_words(&segments);
        for word in &mut words {
            word.start = offset_ms + word.start.clamp(0.0, call.duration_ms);
            word.end = (offset_ms + word.end.clamp(0.0, call.duration_ms)).max(word.start);
        }
        Ok(words)
    }
}

/// Transcribe an already-buffered 16 kHz fixture with bounded inference chunks.
/// Production package transcription streams its input in transcribe.rs.
pub fn transcribe(samples: &[f32], model: Model, progress: &Arc<Progress>) -> Result<Vec<Word>> {
    progress.check()?;
    anyhow::ensure!(
        samples.iter().all(|sample| sample.is_finite()),
        "non-finite transcription audio"
    );
    let mut session = None;
    let mut words = Vec::new();
    let total_ms = (samples.len() as f64 / 16.0).max(1.0);
    for pair in crate::transcribe::split_points(samples).windows(2) {
        progress.check()?;
        let chunk = &samples[pair[0]..pair[1]];
        if has_signal(chunk) {
            if session.is_none() {
                session = Some(Session::new(model, progress)?);
            }
            words.extend(session.as_mut().unwrap().chunk(
                chunk,
                pair[0] as f64 / 16.0,
                total_ms,
            )?);
        }
    }
    progress.check()?;
    progress.permille.store(1000, Ordering::Relaxed);
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "uses the downloaded Whisper base model to check actual inference cancellation"]
    fn cancellation_interrupts_real_inference() {
        assert!(
            Model::Base.is_downloaded(),
            "download the Whisper base model before this check"
        );
        let progress = Arc::new(Progress::default());
        let mut session = Session::new(Model::Base, &progress).unwrap();
        let signal = progress.clone();
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            signal.cancel();
        });
        let samples = vec![0.1; 480000];
        let started = std::time::Instant::now();
        assert!(
            session
                .chunk(&samples, 0.0, 30000.0)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        cancel.join().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn silence_trimming_retains_context_and_original_time() {
        let mut samples = vec![0.0; 48000];
        samples[16000..32000].fill(0.1);
        assert_eq!(speech_range(&samples), Some(8000..40000));
        assert_eq!(speech_range(&[0.1; 100]), Some(0..100));
        assert_eq!(speech_range(&[0.0; 100]), None);
    }

    #[test]
    fn callbacks_map_chunk_progress_and_cancel_without_owning_closures() {
        let progress = Arc::new(Progress::default());
        let call = CallProgress {
            progress: &progress,
            offset_ms: 1800000.0,
            duration_ms: 55000.0,
            total_ms: 3600000.0,
        };
        let data = (&call as *const CallProgress<'_>).cast_mut().cast();
        unsafe {
            report_progress(std::ptr::null_mut(), std::ptr::null_mut(), 50, data);
        }
        assert_eq!(progress.permille.load(Ordering::Relaxed), 507);
        assert!(!unsafe { should_abort(data) });
        progress.cancel();
        assert!(unsafe { should_abort(data) });
        assert!(unsafe { should_abort(std::ptr::null_mut()) });
        assert_eq!(Arc::strong_count(&progress), 1);
    }

    #[test]
    fn silence_invalid_samples_and_cancellation_need_no_model_download() {
        let progress = Arc::new(Progress::default());
        assert!(
            transcribe(&[0.0; 160], Model::Medium, &progress)
                .unwrap()
                .is_empty()
        );
        assert!(transcribe(&[f32::NAN], Model::Medium, &progress).is_err());
        progress.cancel();
        assert!(
            download(Model::Medium, &progress)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(transcribe(&[0.1; 160], Model::Medium, &progress).is_err());
    }

    #[test]
    fn merges_word_pieces_and_drops_annotations() {
        let segs = vec![
            (0.0, 300.0, " Hello".to_string()),
            (300.0, 600.0, " world".to_string()),
            (600.0, 650.0, ",".to_string()),
            (700.0, 900.0, " [BLANK_AUDIO]".to_string()),
            (1000.0, 1200.0, " don".to_string()),
            (1200.0, 1300.0, "'t".to_string()),
        ];
        let w = merge_words(&segs);
        let texts: Vec<_> = w.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Hello", "world,", "don't"]);
        assert_eq!((w[1].start, w[1].end), (300.0, 650.0));
        assert_eq!((w[2].start, w[2].end), (1000.0, 1300.0));
    }
}
