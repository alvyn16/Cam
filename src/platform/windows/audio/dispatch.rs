//! Keep resampling and consumers off the WASAPI capture thread.
use super::convert::Converter;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

const BYTE_BUDGET: usize = 8 * 1024 * 1024;
const PACKETS: usize = 512;
struct Packet {
    samples: Vec<f32>,
    at: f64,
    bytes: Arc<AtomicUsize>,
}
impl Drop for Packet {
    fn drop(&mut self) {
        self.bytes
            .fetch_sub(self.samples.len() * 4, Ordering::AcqRel);
    }
}
#[derive(Clone)]
pub(super) struct Input {
    sender: mpsc::SyncSender<Packet>,
    bytes: Arc<AtomicUsize>,
    error: Arc<Mutex<Option<String>>>,
}
impl Input {
    pub fn push(&self, samples: Vec<f32>, at: f64) {
        let bytes = samples.len() * 4;
        let reserve = self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                queued
                    .checked_add(bytes)
                    .filter(|total| *total <= BYTE_BUDGET)
            });
        let failed = if reserve.is_err() {
            true
        } else {
            self.sender
                .try_send(Packet {
                    samples,
                    at,
                    bytes: self.bytes.clone(),
                })
                .is_err()
        };
        if failed {
            let mut error = self.error.lock().unwrap();
            if error.is_none() {
                *error = Some(
                    "Audio processing cannot keep up; recording stopped to avoid losing audio."
                        .into(),
                );
            }
        }
    }
}
/// Drop the native stream (and its Input clone) before dropping Processor.
pub(super) struct Processor {
    input: Option<Input>,
    worker: Option<std::thread::JoinHandle<()>>,
    ready: mpsc::Receiver<()>,
}
impl Processor {
    pub fn new(
        rate: u32,
        channels: usize,
        mut callback: Box<dyn FnMut(&[f32], f64) + Send>,
        error: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Packet>(PACKETS);
        let input = Input {
            sender,
            bytes: Default::default(),
            error,
        };
        let (ready, initialized) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let mut converter = Converter::new(rate, channels);
            let mut ready = Some(ready);
            let mut stable_frames = 0u64;
            let mut previous_end: Option<f64> = None;
            while let Ok(packet) = receiver.recv() {
                if ready.is_some() {
                    if previous_end.is_some_and(|end| (packet.at - end).abs() > 0.002) {
                        stable_frames = 0;
                    }
                    stable_frames += (packet.samples.len() / channels) as u64;
                    previous_end = Some(
                        packet.at + packet.samples.len() as f64 / channels as f64 / rate as f64,
                    );
                    if stable_frames >= rate as u64 / 2 {
                        let _ = ready.take().unwrap().try_send(());
                    }
                }
                let (at, samples) = converter.push(&packet.samples, packet.at);
                if !samples.is_empty() {
                    callback(&samples, at);
                }
            }
        });
        Self {
            input: Some(input),
            worker: Some(worker),
            ready: initialized,
        }
    }
    pub fn input(&self) -> Input {
        self.input.as_ref().unwrap().clone()
    }
    pub fn wait_ready(&self) -> anyhow::Result<()> {
        self.ready
            .recv_timeout(std::time::Duration::from_secs(4))
            .map_err(|error| {
                anyhow::anyhow!("Microphone did not deliver stable audio during startup: {error}")
            })
    }
}
impl Drop for Processor {
    fn drop(&mut self) {
        self.input.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_requires_half_a_second_of_continuous_device_samples() {
        let (sent, received) = mpsc::sync_channel(1);
        let processor = Processor::new(
            48_000,
            1,
            Box::new(move |_, _| {
                sent.send(()).unwrap();
            }),
            Default::default(),
        );
        let input = processor.input();
        for i in 0..25 {
            input.push(vec![0.0; 480], i as f64 / 100.0);
            received
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
        }
        // A real missing packet resets the startup stability window.
        for i in 0..49 {
            input.push(vec![0.0; 480], 1.0 + i as f64 / 100.0);
            received
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
        }
        assert!(processor.ready.try_recv().is_err());
        input.push(vec![0.0; 480], 1.49);
        processor.wait_ready().unwrap();
        received
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        drop(input);
        drop(processor);
    }
    #[test]
    fn slow_consumer_does_not_block_capture_and_drop_drains_in_order() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let output = received.clone();
        let error = Arc::new(Mutex::new(None));
        let processor = Processor::new(
            48_000,
            1,
            Box::new(move |samples, at| {
                std::thread::sleep(std::time::Duration::from_millis(3));
                output.lock().unwrap().push((at, samples.len(), samples[0]));
            }),
            error.clone(),
        );
        let input = processor.input();
        let start = std::time::Instant::now();
        for i in 0..128 {
            input.push(vec![i as f32 / 128.0; 480], 10.0 + i as f64 / 100.0);
        }
        assert!(
            start.elapsed() < std::time::Duration::from_millis(100),
            "capture waited for the consumer"
        );
        let charged = input.bytes.clone();
        drop(input);
        drop(processor);
        assert!(error.lock().unwrap().is_none());
        let received = received.lock().unwrap();
        assert_eq!(received.len(), 128);
        for (i, (at, count, sample)) in received.iter().enumerate() {
            assert!((*at - (10.0 + i as f64 / 100.0)).abs() < 1e-9);
            assert_eq!((*count, *sample), (960, i as f32 / 128.0));
        }
        assert_eq!(charged.load(Ordering::Acquire), 0);
    }
    #[test]
    fn stalled_consumer_has_a_byte_budget_and_reports_overload() {
        let (entered, started) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let mut first = true;
        let error = Arc::new(Mutex::new(None));
        let processor = Processor::new(
            48_000,
            2,
            Box::new(move |_, _| {
                if first {
                    first = false;
                    entered.send(()).unwrap();
                    resume.recv().unwrap();
                }
            }),
            error.clone(),
        );
        let input = processor.input();
        input.push(vec![0.1; 4096], 0.0);
        started.recv().unwrap();
        for i in 1..=BYTE_BUDGET / (4096 * 4) {
            input.push(vec![0.1; 4096], i as f64 * 2048.0 / 48_000.0);
        }
        assert_eq!(input.bytes.load(Ordering::Acquire), BYTE_BUDGET);
        assert!(
            error
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .contains("cannot keep up")
        );
        let charged = input.bytes.clone();
        release.send(()).unwrap();
        drop(input);
        drop(processor);
        assert_eq!(charged.load(Ordering::Acquire), 0);
    }
}
