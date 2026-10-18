//! Cancellable, bounded file-audio decoding. The decoder cannot run ahead of
//! two 32 KiB blocks, and drop kills/reaps it before joining the pipe reader.
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{ErrorKind, Read},
    path::Path,
    process::{Child, Stdio},
    sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel},
    thread::JoinHandle,
    time::Duration,
};

const BYTES: usize = 32768;
type Chunk = std::result::Result<Vec<f32>, String>;

pub struct AudioStream {
    decoder: Child,
    receiver: Option<Receiver<Chunk>>,
    reader: Option<JoinHandle<()>>,
    log: super::mux::Pending,
    description: String,
    finished: bool,
}
impl AudioStream {
    pub fn open(source: &Path, track: usize, rate: u32, channels: usize) -> Result<Self> {
        ensure!(
            rate > 0 && rate <= 192000 && matches!(channels, 1 | 2),
            "invalid audio decode format"
        );
        let source = source
            .canonicalize()
            .with_context(|| format!("opening audio {}", source.display()))?;
        ensure!(source.is_file(), "audio source is not a file");
        let description = format!("{} audio track {track}", source.display());
        let log = super::mux::temporary(&std::env::temp_dir().join("cam-audio.log"), "decode-log")?;
        let mut decoder = super::ffmpeg::command("ffmpeg")
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(source)
            .args([
                "-map",
                &format!("0:a:{track}"),
                "-vn",
                "-ac",
                &channels.to_string(),
                "-ar",
                &rate.to_string(),
                "-f",
                "f32le",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(File::create(log.path())?)
            .spawn()
            .with_context(|| format!("starting decoder for {description}"))?;
        let stdout = decoder.stdout.take().expect("piped decoder stdout");
        let (sender, receiver) = sync_channel(2);
        let reader = match std::thread::Builder::new()
            .name("cam-audio-stream".into())
            .spawn(move || {
                if let Err(error) = read_pcm(stdout, channels, &sender) {
                    let _ = sender.send(Err(format!("{error:#}")));
                }
            }) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = decoder.kill();
                let _ = decoder.wait();
                return Err(error).context("starting audio pipe reader");
            }
        };
        Ok(Self {
            decoder,
            receiver: Some(receiver),
            reader: Some(reader),
            log,
            description,
            finished: false,
        })
    }

    /// Chunks contain complete interleaved frames in the requested format.
    pub fn next_chunk(&mut self, cancelled: &dyn Fn() -> bool) -> Result<Option<Vec<f32>>> {
        if self.finished {
            return Ok(None);
        }
        loop {
            if cancelled() {
                self.stop();
                anyhow::bail!("audio decoding cancelled");
            }
            match self
                .receiver
                .as_ref()
                .context("audio reader is closed")?
                .recv_timeout(Duration::from_millis(20))
            {
                Ok(Ok(samples)) => return Ok(Some(samples)),
                Ok(Err(error)) => {
                    self.stop();
                    anyhow::bail!("decoding {}: {error}", self.description);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let status = self.decoder.wait().context("waiting for audio decoder")?;
                    self.finished = true;
                    self.receiver.take();
                    if let Some(reader) = self.reader.take() {
                        ensure!(reader.join().is_ok(), "audio pipe reader panicked");
                    }
                    if !status.success() {
                        let mut bytes = Vec::new();
                        File::open(self.log.path())?
                            .take(65536)
                            .read_to_end(&mut bytes)?;
                        anyhow::bail!(
                            "decoding {}: {}",
                            self.description,
                            String::from_utf8_lossy(&bytes).trim()
                        );
                    }
                    ensure!(!cancelled(), "audio decoding cancelled");
                    return Ok(None);
                }
            }
        }
    }
    fn stop(&mut self) {
        // Release a blocked sender before waiting for the reader. Killing the
        // decoder interrupts a reader blocked inside the anonymous pipe.
        self.receiver.take();
        let _ = self.decoder.kill();
        let _ = self.decoder.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.finished = true;
    }
}
impl Drop for AudioStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_pcm(mut input: impl Read, channels: usize, sender: &SyncSender<Chunk>) -> Result<()> {
    let mut bytes = [0u8; BYTES];
    loop {
        let mut count = 0;
        while count < bytes.len() {
            match input.read(&mut bytes[count..]) {
                Ok(0) => break,
                Ok(n) => count += n,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("reading decoded audio"),
            }
        }
        if count == 0 {
            return Ok(());
        }
        ensure!(
            count % (channels * 4) == 0,
            "decoder returned a partial audio frame"
        );
        let samples: Vec<f32> = bytes[..count]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        ensure!(
            samples.iter().all(|sample| sample.is_finite()),
            "decoded audio contains non-finite samples"
        );
        if sender.send(Ok(samples)).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fragmented(std::io::Cursor<Vec<u8>>);
    impl Read for Fragmented {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = output.len().min(7);
            self.0.read(&mut output[..count])
        }
    }
    #[test]
    fn fragmented_reads_preserve_samples_and_bound_chunks() {
        let samples: Vec<f32> = (0..50002).map(|i| (i as f32 * 0.03).sin()).collect();
        let bytes: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let (sender, receiver) = sync_channel(2);
        let worker = std::thread::spawn(move || {
            read_pcm(Fragmented(std::io::Cursor::new(bytes)), 2, &sender)
        });
        let mut result = Vec::new();
        for chunk in receiver {
            let chunk = chunk.unwrap();
            assert!(chunk.len() <= BYTES / 4 && chunk.len() % 2 == 0);
            result.extend(chunk);
        }
        worker.join().unwrap().unwrap();
        assert_eq!(result, samples);
    }
    #[test]
    fn invalid_frames_and_nonfinite_audio_are_rejected() {
        for bytes in [vec![0; 7], f32::NAN.to_le_bytes().to_vec()] {
            let (sender, _receiver) = sync_channel(2);
            assert!(read_pcm(std::io::Cursor::new(bytes), 1, &sender).is_err());
        }
    }
    fn directory() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cam-audio-stream-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&dir).unwrap();
        dir
    }
    #[test]
    fn compressed_stream_matches_reference_and_honors_format() {
        let dir = directory();
        let source = dir.join("source.mp3");
        super::super::ffmpeg::output(
            super::super::ffmpeg::command("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=880:duration=0.7:sample_rate=44100",
                    "-ac",
                    "2",
                ])
                .arg(&source),
        )
        .unwrap();
        let reference = super::super::read_audio(&source, 0).unwrap();
        let mut stream = AudioStream::open(&source, 0, 48000, 2).unwrap();
        let log = stream.log.path().to_owned();
        let mut samples = Vec::new();
        while let Some(chunk) = stream.next_chunk(&|| false).unwrap() {
            samples.extend(chunk);
        }
        assert_eq!(
            samples.as_slice(),
            bytemuck::cast_slice::<[f32; 2], f32>(&reference)
        );
        drop(stream);
        assert!(!log.exists());
        let mut mono = AudioStream::open(&source, 0, 16000, 1).unwrap();
        let mut count = 0;
        while let Some(chunk) = mono.next_chunk(&|| false).unwrap() {
            count += chunk.len();
        }
        assert_eq!(count, 11200);
        assert!(source.is_file());
    }
    #[test]
    fn cancellation_and_decoder_errors_reap_processes_and_keep_sources() {
        let dir = directory();
        let source = dir.join("source.wav");
        super::super::ffmpeg::output(
            super::super::ffmpeg::command("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=48000:cl=stereo",
                    "-t",
                    "2",
                ])
                .arg(&source),
        )
        .unwrap();
        let original = std::fs::read(&source).unwrap();
        let mut stream = AudioStream::open(&source, 0, 48000, 2).unwrap();
        let log = stream.log.path().to_owned();
        assert!(stream.next_chunk(&|| true).is_err());
        assert!(stream.decoder.try_wait().unwrap().is_some());
        drop(stream);
        assert!(!log.exists());
        let mut invalid = AudioStream::open(&source, 9, 48000, 2).unwrap();
        assert!(
            invalid
                .next_chunk(&|| false)
                .unwrap_err()
                .to_string()
                .contains("audio track 9")
        );
        assert_eq!(std::fs::read(&source).unwrap(), original);
    }
}
