//! Direct compositor-to-native-encoder export. Source video decoding/uploads
//! remain on the established input path until the gpu-decoder slice lands.
use super::{Inputs, Job, ProgressFn, Source, Stats};
use crate::{
    engine::scene::Scene,
    media::{mf_encoder::NativeWriter, mux, writer::WriterSpec},
    project::model::{Codec, ExportFormat},
    render::{Gpu, Renderer},
};
use anyhow::{Context, Result, ensure};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Policy {
    Off,
    Auto,
    Require,
}

/// An export owns its headless device. Explicit destruction also releases
/// backend resources when temporary imported textures held internal references.
struct ScopedGpu(Gpu);
impl Drop for ScopedGpu {
    fn drop(&mut self) {
        self.0.device.destroy();
    }
}
pub(super) fn configured() -> Result<Policy> {
    let policy = match std::env::var("CAM_NATIVE_ENCODER").as_deref() {
        Err(_) => Policy::Auto,
        Ok("off" | "0") => Policy::Off,
        Ok("auto") => Policy::Auto,
        Ok("require" | "1") => Policy::Require,
        Ok(other) => anyhow::bail!("invalid CAM_NATIVE_ENCODER value: {other}"),
    };
    if std::env::var("CAM_ENCODER").as_deref() == Ok("software") {
        ensure!(
            policy != Policy::Require,
            "native encoding requirement conflicts with CAM_ENCODER=software"
        );
        return Ok(Policy::Off);
    }
    Ok(policy)
}

#[cfg(test)]
pub(super) fn try_export(
    job: &Job,
    scene: &mut Scene,
    progress: ProgressFn,
    cancel: &AtomicBool,
    policy: Policy,
) -> Result<Option<Stats>> {
    try_export_in(job, scene, progress, cancel, policy, None)
}

pub(super) fn try_export_in(
    job: &Job,
    scene: &mut Scene,
    progress: ProgressFn,
    cancel: &AtomicBool,
    policy: Policy,
    scratch: Option<&std::path::Path>,
) -> Result<Option<Stats>> {
    if policy == Policy::Off || job.settings.format != ExportFormat::Mp4 {
        return Ok(None);
    }
    let started = Instant::now();
    let settings = job.settings;
    let (width, height) = scene.sizes.render_size(settings.height);
    let codec = settings.codec_for(width, height);
    if codec == Codec::Prores {
        ensure!(
            policy != Policy::Require,
            "native ProRes export is unavailable"
        );
        return Ok(None);
    }
    ensure!(!cancel.load(Ordering::Acquire), "export cancelled");
    let video = mux::temporary(&job.output, "native-video")?;
    let initialize = || -> Result<_> {
        let gpu = ScopedGpu(Gpu::headless_dx12()?);
        let writer = NativeWriter::create(
            video.path(),
            &WriterSpec {
                width,
                height,
                fps: settings.fps,
                codec,
                // Match the fallback's quality mode unless fitting a file size.
                // Audio remains a separate, disk-backed mux input.
                bitrate: settings.bitrate(width, height, scene.duration() / 1000.0),
                quality: (settings.target_mb.is_none() || !settings.fits_size())
                    .then(|| settings.quality.encoder_quality()),
                audio: false,
            },
            &gpu.0,
        )?;
        Ok((gpu, writer))
    };
    let (gpu, mut writer) = match initialize() {
        Ok(value) => value,
        Err(error) if policy == Policy::Auto => {
            eprintln!("Cam native export unavailable; using FFmpeg: {error:#}");
            return Ok(None);
        }
        Err(error) => return Err(error).context("initializing required native GPU export"),
    };
    eprintln!("Cam native GPU export: {}", writer.encoder_name());
    let dir = match &job.source {
        Source::Package(dir) => Some(dir.as_path()),
        Source::Synthetic => None,
    };
    let cache = scratch.map(|path| path.join("audio")).unwrap_or_else(|| {
        dir.map(|dir| dir.join("cache"))
            .unwrap_or_else(|| std::env::temp_dir().join("cam-audio-cache"))
    });
    let cancelled = || cancel.load(Ordering::Acquire);
    let soundtrack = super::stream_audio::render(
        &job.project,
        &job.recording,
        dir,
        &cache,
        &scene.map,
        &cancelled,
    )?;
    let mut renderer = Renderer::new(gpu.0.clone());
    renderer.prepare(&job.project.style, &job.recording, dir);
    let mut inputs = Inputs::open(job)?;
    let total = scene.frame_count();
    let profile = std::env::var_os("CAM_NATIVE_PROFILE").is_some();
    let times: Vec<f64> = (0..total)
        .map(|i| scene.map.to_source(i as f64 * 1000.0 / settings.fps as f64))
        .collect();
    inputs.prefetch(job, &times);
    for index in 0..total {
        ensure!(!cancelled(), "export cancelled");
        let frame = scene.frame(index as f64 * 1000.0 / settings.fps as f64, height as f64);
        inputs.feed_index(&mut renderer, index as usize, frame.source_ms, None);
        let target = writer.render(width, height, |target, encoder| {
            renderer.render(&frame, &job.project.style, target, encoder);
        })?;
        writer.push_cancellable(&target, &cancelled)?;
        if profile && (index == 0 || (index + 1) % 1000 == 0 || index + 1 == total) {
            let (used, budget) = writer.gpu_memory_usage()?;
            eprintln!(
                "native-gpu-memory frames={} local={} budget={}",
                index + 1,
                used,
                budget
            );
        }
        progress((index + 1) as f64 / total as f64);
    }
    writer.finish_cancellable(&cancelled)?;
    mux::publish(
        video.path(),
        soundtrack.as_ref().map(|audio| audio.path()),
        &job.output,
        &cancelled,
    )?;
    if settings.fits_size() {
        if let Some(mb) = settings.target_mb {
            super::fit::apply(&job.output, mb, cancel)?;
        }
    }
    ensure!(!cancelled(), "export cancelled");
    progress(1.0);
    Ok(Some(Stats {
        frames: total,
        width,
        height,
        seconds: started.elapsed().as_secs_f64(),
        video_seconds: scene.duration() / 1000.0,
        bytes: job.output.metadata()?.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{
        Package,
        model::{ExportSettings, Quality, Slice},
    };

    #[test]
    #[ignore = "checks GPU memory and handle release across repeated native export jobs"]
    fn repeated_jobs_release_native_devices() {
        let dir = std::env::temp_dir().join(format!(
            "cam-native-repeat-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&dir).unwrap();
        let monitor_gpu = Gpu::headless_dx12().unwrap();
        let monitor = crate::render::dx12::GpuInterop::new(&monitor_gpu).unwrap();
        let recording = crate::synth::recording(320, 200, 400.0);
        let project = crate::project::model::Project::default();
        let settings = ExportSettings {
            height: 240,
            fps: 10,
            ..Default::default()
        };
        let mut measurements = Vec::new();
        for index in 0..8 {
            let job = Job {
                project: project.clone(),
                recording: recording.clone(),
                source: Source::Synthetic,
                settings: crate::project::model::ExportSettings {
                    codec: if index % 2 == 0 {
                        Codec::H264
                    } else {
                        Codec::Hevc
                    },
                    ..settings
                },
                output: dir.join(format!("{index}.mp4")),
            };
            crate::export::native_worker::try_export(
                &job,
                &mut |_| {},
                &AtomicBool::new(false),
                Policy::Require,
            )
            .unwrap()
            .unwrap();
            let probe = crate::media::ffmpeg::probe(&job.output).unwrap();
            assert_eq!(probe["streams"][0]["nb_frames"].as_str(), Some("4"));
            assert!(
                crate::media::ffmpeg::output(
                    crate::media::ffmpeg::command("ffmpeg")
                        .args(["-v", "error", "-i"])
                        .arg(&job.output)
                        .args(["-f", "null", "NUL"])
                )
                .is_ok()
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut handles = 0;
            unsafe {
                windows62::Win32::System::Threading::GetProcessHandleCount(
                    windows62::Win32::System::Threading::GetCurrentProcess(),
                    &mut handles,
                )
                .unwrap();
            }
            let bytes = monitor.local_memory().unwrap().CurrentUsage;
            eprintln!("After native job {index}: handles={handles}, GPU bytes={bytes}");
            measurements.push((handles, bytes));
        }
        let steady = &measurements[2..];
        let min_handles = steady.iter().map(|m| m.0).min().unwrap();
        let max_handles = steady.iter().map(|m| m.0).max().unwrap();
        let min_bytes = steady.iter().map(|m| m.1).min().unwrap();
        let max_bytes = steady.iter().map(|m| m.1).max().unwrap();
        assert!(
            max_handles - min_handles <= 8,
            "native jobs accumulated handles: {measurements:?}"
        );
        assert!(
            max_bytes - min_bytes <= 32 * 1024 * 1024,
            "native jobs accumulated GPU memory: {measurements:?}"
        );
    }

    #[test]
    #[ignore = "checks full-duration native H.264/HEVC exports fitted to a file size with audio"]
    fn native_file_size_target_retains_video_and_audio() {
        let dir = std::env::temp_dir().join(format!(
            "cam-native-fit-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("Source.cam");
        crate::synth::write_package(&path, 2.0, 640, 400).unwrap();
        crate::synth::add_tone(&path, 880.0).unwrap();
        let package = Package::new(&path);
        let (project, recording) = package.load().unwrap();
        let mut source = crate::media::writer::MovieWriter::create(
            &package.rec("screen.mov"),
            &WriterSpec {
                width: 640,
                height: 400,
                fps: 10,
                codec: Codec::Prores,
                bitrate: 20_000_000,
                quality: None,
                audio: false,
            },
        )
        .unwrap();
        let pixels = source.buffer().unwrap();
        let mut seed = 42u32;
        for _ in 0..20 {
            let mut frame = pixels.bgra.lock().unwrap();
            for pixel in frame.chunks_exact_mut(4) {
                for value in &mut pixel[..3] {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    *value = seed as u8;
                }
                pixel[3] = 255;
            }
            drop(frame);
            source.push_frame(&pixels).unwrap();
        }
        source.finish().unwrap();
        for codec in [Codec::H264, Codec::Hevc] {
            let output = dir.join(format!("{codec:?}.mp4"));
            let settings = ExportSettings {
                height: 400,
                fps: 10,
                codec,
                quality: Quality::Studio,
                target_mb: Some(1),
                ..Default::default()
            };
            let job = Job {
                project: project.clone(),
                recording: recording.clone(),
                source: Source::Package(path.clone()),
                settings,
                output: output.clone(),
            };
            let mut scene = Scene::new(project.clone(), &recording, 10.0);
            try_export(
                &job,
                &mut scene,
                &mut |_| {},
                &AtomicBool::new(false),
                Policy::Require,
            )
            .unwrap()
            .unwrap();
            assert!(output.metadata().unwrap().len() <= 1_000_000);
            let probe = crate::media::ffmpeg::probe(&output).unwrap();
            let video = probe["streams"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["codec_type"] == "video")
                .unwrap();
            assert_eq!(video["nb_frames"], "20");
            assert!(
                (probe["format"]["duration"]
                    .as_str()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
                    - 2.0)
                    .abs()
                    < 0.05
            );
            assert!(
                crate::audiocheck::analyze(&crate::media::read_audio(&output, 0).unwrap(), 880.0)
                    .tone_seconds
                    > 1.8
            );
            crate::media::ffmpeg::output(
                crate::media::ffmpeg::command("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&output)
                    .args(["-f", "null", "NUL"]),
            )
            .unwrap();
        }
        eprintln!("Native file-size artifacts: {}", dir.display());
    }
    #[test]
    #[ignore = "renders edited projects through native GPU H.264/HEVC with audio and cancellation"]
    fn edited_project_audio_and_cancellation_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "cam-native-export-{}-{}",
            std::process::id(),
            crate::util::host_now().to_bits()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("Source.cam");
        crate::synth::write_package(&path, 1.2, 320, 200).unwrap();
        crate::synth::add_tone(&path, 880.0).unwrap();
        let package = Package::new(&path);
        let (mut project, recording) = package.load().unwrap();
        project.timeline.slices = vec![
            Slice {
                start: 0.0,
                end: 600.0,
                speed: 1.0,
                ..Default::default()
            },
            Slice {
                start: 600.0,
                end: 1200.0,
                speed: 2.0,
                ..Default::default()
            },
        ];
        let original_audio = std::fs::read(package.rec("system.wav")).unwrap();
        for codec in [Codec::H264, Codec::Hevc] {
            let output = dir.join(format!("{codec:?}.mp4"));
            let settings = ExportSettings {
                height: 240,
                fps: 20,
                codec,
                quality: Quality::Social,
                ..Default::default()
            };
            let job = Job {
                project: project.clone(),
                recording: recording.clone(),
                source: Source::Package(path.clone()),
                settings,
                output: output.clone(),
            };
            for mode in 0..3 {
                std::fs::write(&output, b"existing user export").unwrap();
                let cancel = std::sync::Arc::new(AtomicBool::new(false));
                let end = std::sync::Arc::new(AtomicBool::new(false));
                let seen = std::sync::Arc::new(AtomicBool::new(false));
                let observer = (mode == 2).then(|| {
                    let (cancel, end, seen, dir) =
                        (cancel.clone(), end.clone(), seen.clone(), dir.clone());
                    std::thread::spawn(move || {
                        while !end.load(Ordering::Acquire) {
                            if std::fs::read_dir(&dir)
                                .unwrap()
                                .flatten()
                                .any(|e| e.file_name().to_string_lossy().contains(".complete."))
                            {
                                seen.store(true, Ordering::Release);
                                cancel.store(true, Ordering::Release);
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    })
                });
                let mut scene = Scene::new(project.clone(), &recording, settings.fps as f64);
                let result = try_export(
                    &job,
                    &mut scene,
                    &mut |progress| {
                        if (mode == 0 && progress >= 0.3) || (mode == 1 && progress >= 1.0) {
                            cancel.store(true, Ordering::Release);
                        }
                    },
                    &cancel,
                    Policy::Require,
                );
                end.store(true, Ordering::Release);
                if let Some(observer) = observer {
                    observer.join().unwrap();
                }
                assert!(format!("{:#}", result.unwrap_err()).contains("cancel"));
                if mode == 2 {
                    assert!(
                        seen.load(Ordering::Acquire),
                        "did not reach mux cancellation"
                    );
                }
                assert_eq!(std::fs::read(&output).unwrap(), b"existing user export");
                assert!(
                    std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e
                        .file_name()
                        .to_string_lossy()
                        .contains("native-video")
                        && !e.file_name().to_string_lossy().contains(".complete.")),
                    "native export staging remains"
                );
            }
            let mut scene = Scene::new(project.clone(), &recording, settings.fps as f64);
            let stats = try_export(
                &job,
                &mut scene,
                &mut |_| {},
                &AtomicBool::new(false),
                Policy::Require,
            )
            .unwrap()
            .unwrap();
            let probe = crate::media::ffmpeg::probe(&output).unwrap();
            let video = probe["streams"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["codec_type"] == "video")
                .unwrap();
            assert_eq!(video["nb_frames"], stats.frames.to_string());
            assert_eq!(stats.frames, 18);
            assert!((stats.video_seconds - 0.9).abs() < 0.001);
            let audio = crate::media::read_audio(&output, 0).unwrap();
            let level = crate::audiocheck::analyze(&audio, 880.0);
            // The second clip is played at 2x, including the existing mixer
            // behavior of doubling its tone frequency.
            assert!(
                level.tone_seconds >= 0.55 && level.tone_seconds <= 0.65,
                "{level:?}"
            );
            let fast = crate::audiocheck::analyze(&audio[28_800..], 1760.0);
            assert!(fast.tone_seconds >= 0.25, "{fast:?}");
            assert!((level.tone_db + 23.0).abs() < 1.5, "{level:?}");
            assert_eq!(
                std::fs::read(package.rec("system.wav")).unwrap(),
                original_audio
            );
        }
        eprintln!("Edited native export artifacts: {}", dir.display());
    }
}
