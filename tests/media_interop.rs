//! Validate with independent tools, never the application's own writer format.
use cam::media::{
    self,
    writer::{MovieWriter, WriterSpec},
};
use cam::project::model::Codec;
use std::sync::Arc;

#[test]
fn capture_codec_options_reach_screen_and_camera_encoders() {
    use cam::capture::{
        compose::{Corner, PipSpec, Scene, Shape},
        record::{MicBackend, Options, Session},
        writer::{AudioLayout, Codec as CaptureCodec, Sink, VideoSpec},
    };
    let dir = std::env::temp_dir().join(format!(
        "cam-capture-codecs-{}-{}",
        std::process::id(),
        cam::util::host_now().to_bits()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for (codec, expected) in [
        (CaptureCodec::H264, "h264"),
        (CaptureCodec::Hevc, "hevc"),
        (CaptureCodec::Prores, "prores"),
    ] {
        let screen = dir.join(format!("screen-{expected}.mov"));
        let session = Session::start_pattern(Options {
            output: screen.clone(),
            size: (64, 48),
            fps: 10,
            codec,
            bitrate: 1_000_000,
            quality: Some(0.8),
            system_audio: false,
            mic: None,
            webcam: None,
            pip: PipSpec {
                size: 0.2,
                corner: Corner::BottomRight,
                shape: Shape::Rounded,
                mirror: false,
            },
            hide_desktop_icons: false,
            hide_dock: false,
            exclude_own_audio: false,
            mic_backend: MicBackend::Auto,
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.has_frames() {
            assert!(!session.failed(), "{:?}", session.error());
            assert!(
                std::time::Instant::now() < deadline,
                "pattern did not start"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(110));
        session.finish().unwrap();
        let camera = dir.join(format!("camera-{expected}.mov"));
        let mut sink = Sink::new(
            Some(&camera),
            &VideoSpec {
                width: 64,
                height: 48,
                fps: 10,
                codec,
                bitrate: 1_000_000,
                quality: Some(0.8),
                pixel_format: cam::util::PIX_BGRA,
            },
            false,
            false,
            AudioLayout::Stems,
            [0.0; 2],
            None,
            Scene::Cam,
            None,
        )
        .unwrap();
        sink.push_video(&cam::capture::grab::Image {
            width: 64,
            height: 48,
            bgra: cam::synth::screen_bgra(64, 48, 0.0),
        });
        sink.finish(sink.t0_secs().unwrap() + 0.2).unwrap();
        for path in [&screen, &camera] {
            let probe = media::ffmpeg::probe(path).unwrap();
            assert_eq!(
                probe["streams"][0]["codec_name"],
                expected,
                "{}",
                path.display()
            );
            assert!(
                media::VideoReader::open(path)
                    .unwrap()
                    .next_frame()
                    .is_some()
            );
        }
    }
    // Keep fixtures for independent inspection alongside the test log.
    println!("Capture codec fixtures: {}", dir.display());
}

#[test]
fn standard_video_preserves_moving_frames_and_audio() {
    let dir = std::env::temp_dir().join(format!("cam-interop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (codec, extension, expected) in [
        (Codec::H264, "mp4", "h264"),
        (Codec::Hevc, "mp4", "hevc"),
        (Codec::Prores, "mov", "prores"),
    ] {
        let path = dir.join(format!("{expected}.{extension}"));
        let mut writer = MovieWriter::create(
            &path,
            &WriterSpec {
                width: 64,
                height: 48,
                fps: 10,
                codec,
                bitrate: 1_000_000,
                quality: Some(0.95),
                audio: true,
            },
        )
        .unwrap();
        let pb = writer.buffer().unwrap();
        for i in 0..10 {
            let color = if i < 5 {
                [0, 0, 255, 255]
            } else {
                [255, 0, 0, 255]
            };
            *pb.bgra.lock().unwrap() = color.repeat(64 * 48);
            writer.push_frame(&pb).unwrap();
        }
        writer.set_soundtrack(Arc::new(
            (0..48000)
                .map(|i| {
                    let v = (std::f32::consts::TAU * 880.0 * i as f32 / 48000.0).sin() * 0.1;
                    [v, v]
                })
                .collect(),
        ));
        writer.finish().unwrap();
        let metadata = media::ffmpeg::probe(&path).unwrap();
        let streams = metadata["streams"].as_array().unwrap();
        let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
        assert_eq!(video["codec_name"], expected);
        assert_eq!(video["nb_frames"], "10");
        let mut source = media::FrameSource::open(&path).unwrap();
        let first = source.frame_at(0.0).unwrap().bgra.clone();
        let last = source.frame_at(800.0).unwrap().bgra.clone();
        assert!(first[2] > 200 && first[0] < 30, "first frame red");
        assert!(last[0] > 200 && last[2] < 30, "later frame blue");
        let audio = media::read_audio(&path, 0).unwrap();
        assert!(cam::audiocheck::analyze(&audio, 880.0).tone_seconds > 0.8);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn screen_filename_does_not_generate_synthetic_footage() {
    let dir = std::env::temp_dir().join(format!("cam-filename-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("screen.mov");
    std::fs::write(&path, b"not a movie").unwrap();
    assert!(media::VideoReader::open(&path).is_err());
    assert!(media::read_audio(&path, 0).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn compressed_audio_is_resampled_and_tracks_are_counted() {
    let dir = std::env::temp_dir().join(format!("cam-resample-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.mp3");
    media::ffmpeg::output(
        media::ffmpeg::command("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=44100:duration=1",
                "-c:a",
                "libmp3lame",
            ])
            .arg(&path),
    )
    .unwrap();
    let audio = media::read_audio(&path, 0).unwrap();
    assert!((audio.len() as i64 - 48000).abs() < 100);
    assert_eq!(media::audio_track_count(&path).unwrap(), 1);
    assert!(media::read_audio(&path, 1).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pause_clock_excludes_paused_time() {
    let mut clock = cam::capture::audio::Clock {
        start: Some(10.0),
        ..Default::default()
    };
    assert_eq!(clock.elapsed(12.0), 2.0);
    clock.pause = Some(12.0);
    assert_eq!(clock.elapsed(20.0), 2.0);
    clock.pause = None;
    clock.paused = 8.0;
    assert_eq!(clock.elapsed(21.0), 3.0);
}

#[test]
fn cancelled_writer_preserves_existing_destination() {
    let path = std::env::temp_dir().join(format!("cam-existing-{}.mp4", std::process::id()));
    std::fs::write(&path, b"existing user file").unwrap();
    let writer = MovieWriter::create(
        &path,
        &WriterSpec {
            width: 64,
            height: 48,
            fps: 10,
            codec: Codec::H264,
            bitrate: 1_000_000,
            quality: Some(0.95),
            audio: false,
        },
    )
    .unwrap();
    writer.cancel();
    assert_eq!(std::fs::read(&path).unwrap(), b"existing user file");
    let _ = std::fs::remove_file(path);
}

#[test]
fn cancellation_during_file_soundtrack_mux_preserves_destination_and_import() {
    let dir = std::env::temp_dir().join(format!(
        "cam-mux-cancel-{}-{}",
        std::process::id(),
        cam::util::host_now().to_bits()
    ));
    std::fs::create_dir(&dir).unwrap();
    let output = dir.join("existing.mp4");
    let input = dir.join("source.wav");
    std::fs::write(&output, b"existing user export").unwrap();
    cam::app::player::write_wav(&input, &vec![[0.05; 2]; 48000]).unwrap();
    let mut writer = MovieWriter::create(
        &output,
        &WriterSpec {
            width: 64,
            height: 48,
            fps: 10,
            codec: Codec::H264,
            bitrate: 1_000_000,
            quality: Some(0.8),
            audio: true,
        },
    )
    .unwrap();
    writer.set_soundtrack_file(&input);
    writer.push_frame(&writer.buffer().unwrap()).unwrap();
    let mux_seen = std::cell::Cell::new(false);
    let error = writer
        .finish_cancellable(&|| {
            let running = dir.read_dir().unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".complete.")
            });
            mux_seen.set(running);
            running
        })
        .unwrap_err();
    assert!(mux_seen.get(), "the final mux stage was reached");
    assert!(error.to_string().contains("cancelled"), "{error:#}");
    assert_eq!(std::fs::read(&output).unwrap(), b"existing user export");
    assert_eq!(media::read_audio(&input, 0).unwrap().len(), 48000);
    assert_eq!(
        dir.read_dir().unwrap().count(),
        2,
        "temporary media files were removed"
    );
}

#[test]
fn duplicate_recording_name_preserves_existing_project() {
    let dir = std::env::temp_dir().join(format!("cam-duplicate-{}", std::process::id()));
    let package = dir.join("Existing.cam");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("sentinel"), b"keep this project").unwrap();
    let result = cam::recorder::Recorder::start(cam::recorder::Options {
        dir: dir.clone(),
        name: Some("Existing".into()),
        system_audio: false,
        ..Default::default()
    });
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(package.join("sentinel")).unwrap(),
        b"keep this project"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cancelling_export_preserves_existing_mp4_and_gif() {
    use cam::{
        export::{Job, Source},
        project::model::{ExportFormat, ExportSettings},
    };
    let dir = std::env::temp_dir().join(format!("cam-cancel-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (format, extension) in [(ExportFormat::Mp4, "mp4"), (ExportFormat::Gif, "gif")] {
        let output = dir.join(format!("existing.{extension}"));
        std::fs::write(&output, b"keep the old export").unwrap();
        let recording = cam::synth::recording(320, 200, 300.0);
        let project = cam::project::new_project("cancel", &recording);
        let job = Job {
            project,
            recording,
            source: Source::Synthetic,
            settings: ExportSettings {
                format,
                height: 240,
                fps: 10,
                ..Default::default()
            },
            output: output.clone(),
        };
        assert!(
            cam::export::run(&job, &mut |_| {}, &std::sync::atomic::AtomicBool::new(true)).is_err()
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"keep the old export");
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn invalid_export_timeline_is_rejected_without_replacing_destination() {
    let dir = std::env::temp_dir().join(format!(
        "cam-invalid-export-{}-{}",
        std::process::id(),
        cam::util::host_now().to_bits()
    ));
    std::fs::create_dir(&dir).unwrap();
    let output = dir.join("keep.mp4");
    std::fs::write(&output, b"keep").unwrap();
    for (duration, fps) in [
        (0.0, 10),
        (-1.0, 10),
        (f64::INFINITY, 10),
        (f64::NAN, 10),
        (1000.0, 0),
    ] {
        let mut recording = cam::synth::recording(64, 48, 0.0);
        recording.meta.duration_ms = duration;
        let job = cam::export::Job {
            recording,
            project: Default::default(),
            source: cam::export::Source::Synthetic,
            settings: cam::project::model::ExportSettings {
                fps,
                height: 64,
                ..Default::default()
            },
            output: output.clone(),
        };
        let error = cam::export::run(
            &job,
            &mut |_| {},
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("invalid export timeline"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"keep");
    }
    assert_eq!(dir.read_dir().unwrap().count(), 1);
}

#[test]
fn file_size_target_keeps_the_whole_video() {
    for codec in [Codec::H264, Codec::Hevc] {
        let path =
            std::env::temp_dir().join(format!("cam-size-{}-{codec:?}.mp4", std::process::id()));
        let mut writer = MovieWriter::create(
            &path,
            &WriterSpec {
                width: 640,
                height: 400,
                fps: 10,
                codec,
                bitrate: 20_000_000,
                quality: Some(1.0),
                audio: false,
            },
        )
        .unwrap();
        let frame = writer.buffer().unwrap();
        let mut seed = 42u32;
        for _ in 0..20 {
            let mut bytes = frame.bgra.lock().unwrap();
            for pixel in bytes.chunks_exact_mut(4) {
                for value in &mut pixel[..3] {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    *value = seed as u8;
                }
                pixel[3] = 255;
            }
            drop(bytes);
            writer.push_frame(&frame).unwrap();
        }
        writer.finish().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000_000);
        cam::export::fit::apply(&path, 1, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() <= 1_000_000);
        let p = media::ffmpeg::probe(&path).unwrap();
        let duration = p["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        assert!((duration - 2.0).abs() < 0.1);
        let _ = std::fs::remove_file(path);
    }
}
