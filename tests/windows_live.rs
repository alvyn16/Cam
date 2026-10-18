//! Explicit opt-in hardware checks: cargo test --test windows_live -- --ignored --test-threads=1
use cam::{
    engine::geom::Rect,
    recorder::{Options, Recorder, Target},
};
use std::{sync::atomic::AtomicBool, time::Duration};

#[test]
#[ignore = "uses the connected webcam, microphone and speakers"]
fn capture_audio_camera_pause_and_export_round_trip() {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let preview = cam::capture::preview::LivePreview::start(
        &cam::capture::preview::Source::Camera { id: None },
        720,
        30,
        false,
        || {},
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(frame) = preview.take_frame() {
            assert!(frame.width > 0 && frame.height > 0);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "camera preview timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(preview);
    let dir = std::env::temp_dir().join(format!("cam-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut recorder = Recorder::start(Options {
        target: Target::Area {
            display: 0,
            rect: Rect::new(100.0, 100.0, 320.0, 200.0),
        },
        dir: dir.clone(),
        name: Some("Live".into()),
        fps: 30,
        mic: Some(None),
        camera: Some(None),
        exclude_own_audio: false, // the round-trip test tone belongs to this process
        ..Default::default()
    })
    .unwrap();
    let device = cpal::default_host().default_output_device().unwrap();
    let config = device.default_output_config().unwrap().config();
    let rate = config.sample_rate as f64;
    let channels = config.channels as usize;
    let mut phase = 0.0f64;
    let tone = device
        .build_output_stream(
            config,
            move |out: &mut [f32], _: &_| {
                for f in out.chunks_mut(channels) {
                    let sample =
                        (std::f64::consts::TAU * 1234.0 * phase / rate).sin() as f32 * 0.04;
                    f.fill(sample);
                    phase += 1.0;
                }
            },
            |e| eprintln!("test tone: {e}"),
            None,
        )
        .unwrap();
    tone.play().unwrap();
    std::thread::sleep(Duration::from_millis(600));
    let preview_frame = recorder
        .take_camera_preview()
        .expect("recording camera preview frame");
    assert_eq!(
        preview_frame.bgra.len(),
        (preview_frame.width * preview_frame.height * 4) as usize
    );
    recorder.pause();
    let paused = recorder.elapsed();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        recorder.take_camera_preview().is_some(),
        "camera monitor remains live while paused"
    );
    assert!((recorder.elapsed() - paused).abs() < 0.03);
    recorder.resume();
    std::thread::sleep(Duration::from_millis(600));
    drop(tone);
    let path = recorder.stop().unwrap();
    let package = cam::project::Package::new(&path);
    let (project, recording) = package.load().unwrap();
    let video = cam::media::VideoReader::open(&package.rec("screen.mov")).unwrap();
    assert_eq!(video.size, (320, 200));
    assert_eq!(
        cam::media::ffmpeg::probe(&package.rec("screen.mov")).unwrap()["streams"][0]["codec_name"],
        "hevc"
    );
    assert!(video.duration_ms > 1000.0);
    let system = cam::media::read_audio(&package.rec("system.wav"), 0).unwrap();
    let level = cam::audiocheck::analyze(&system, 1234.0);
    assert!(level.tone_seconds > 0.8, "{level:?}");
    let mic = cam::media::read_audio(&package.rec("mic.wav"), 0).unwrap();
    assert!(mic.len() > 48000);
    assert!(recording.meta.camera.is_some());
    let camera = cam::media::VideoReader::open(&package.rec("camera.mov")).unwrap();
    assert_eq!(
        cam::media::ffmpeg::probe(&package.rec("camera.mov")).unwrap()["streams"][0]["codec_name"],
        "h264"
    );
    assert!(camera.duration_ms > 500.0);
    let output = dir.join("live.mp4");
    cam::export::run(
        &cam::export::Job {
            project,
            recording,
            source: cam::export::Source::Package(path),
            settings: cam::project::model::ExportSettings {
                height: 240,
                fps: 10,
                ..Default::default()
            },
            output: output.clone(),
        },
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    let audio = cam::media::read_audio(&output, 0).unwrap();
    assert!(cam::audiocheck::analyze(&audio, 1234.0).tone_seconds > 0.7);
    let voice =
        cam::app::voiceover::VoiceRecorder::start(&package.root, "voice-test.wav", 0.0).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert!(voice.stop().unwrap() > 300.0);
    assert!(
        cam::media::read_audio(&package.rec("voice-test.wav"), 0)
            .unwrap()
            .len()
            > 14000
    );
    let cancelled = Recorder::start(Options {
        target: Target::Area {
            display: 0,
            rect: Rect::new(100.0, 100.0, 320.0, 200.0),
        },
        dir: dir.clone(),
        name: Some("Cancelled".into()),
        system_audio: false,
        fps: 10,
        ..Default::default()
    })
    .unwrap();
    let cancelled_path = cancelled.package.root.clone();
    std::thread::sleep(Duration::from_millis(120));
    cancelled.cancel();
    assert!(!cancelled_path.exists());
    println!("Live artifacts: {}", dir.display());
}

#[test]
#[ignore = "opens native recording controls and registers a global shortcut"]
fn native_controls_and_hotkey_conflicts() {
    use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
    let mut bar = cam::app::recbar::RecordBar::new(0, None, "Ctrl+Shift+R", "Ctrl+Shift+P");
    let hwnd = HWND(bar.window_number() as _);
    assert!(unsafe { IsWindowVisible(hwnd) }.as_bool());
    assert!(bar.draggable());
    bar.update(cam::app::recbar::State::Recording {
        clock: "00:05".into(),
        paused: true,
    });
    unsafe {
        SetWindowPos(hwnd, None, 300, 200, 0, 0, SWP_NOSIZE | SWP_NOZORDER).unwrap();
    }
    assert_eq!(bar.origin(), cam::app::recbar::Point::new(300.0, 200.0));
    let combo =
        cam::app::hotkey::Combo::new(0x7b, cam::app::hotkey::CONTROL | cam::app::hotkey::SHIFT);
    cam::app::hotkey::bind(101, combo, || {}).unwrap();
    assert!(cam::app::hotkey::bind(102, combo, || {}).is_err());
    cam::app::hotkey::unbind(101);
    let mut tray = cam::app::menubar::MenuBar::new();
    tray.update(&cam::app::menubar::View {
        recording: true,
        paused: false,
        finishing: false,
        clock: "00:05",
    });
    assert!(tray.title().unwrap().contains("00:05"));
}

#[test]
#[ignore = "opens a focused test window and injects keys to verify native logging and badges"]
fn keyboard_input_and_cursor_images_round_trip() {
    use windows::{
        Win32::{
            Foundation::*,
            UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
        },
        core::w,
    };
    fn key(code: u16, up: bool) {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(code),
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    ..Default::default()
                },
            },
        };
        assert_eq!(
            unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) },
            1
        );
    }
    fn wait(ms: u64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    struct Window {
        hwnd: HWND,
        previous: HWND,
        cursor: POINT,
    }
    impl Drop for Window {
        fn drop(&mut self) {
            for code in [0xa2, 0x11, 0x87, 0x0d] {
                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(code),
                            dwFlags: KEYEVENTF_KEYUP,
                            ..Default::default()
                        },
                    },
                };
                unsafe {
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                }
            }
            unsafe {
                let _ = DestroyWindow(self.hwnd);
                let _ = SetForegroundWindow(self.previous);
                let _ = SetCursorPos(self.cursor.x, self.cursor.y);
            }
        }
    }
    let previous = unsafe { GetForegroundWindow() };
    let mut cursor = POINT::default();
    unsafe {
        GetCursorPos(&mut cursor).unwrap();
    }
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("Cam keyboard verification"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            300,
            200,
            360,
            140,
            None,
            None,
            None,
            None,
        )
        .unwrap()
    };
    let window = Window {
        hwnd,
        previous,
        cursor,
    };
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        SetCursorPos(350, 250).unwrap();
    }
    wait(100);
    assert_eq!(
        unsafe { GetForegroundWindow() },
        hwnd,
        "test window must own focus before injecting keys"
    );
    let dir = std::env::temp_dir().join(format!(
        "cam-live-input-{}-{}",
        std::process::id(),
        cam::util::host_now().to_bits()
    ));
    std::fs::create_dir(&dir).unwrap();
    let logger = cam::recorder::input::InputLogger::start(dir.join("cursors"), None, true);
    wait(100);
    key(0xa2, false);
    wait(60);
    key(0x87, false);
    wait(60);
    key(0x87, true);
    wait(60);
    key(0xa2, true);
    wait(60);
    key(0x0d, false);
    wait(60);
    key(0x0d, true);
    wait(60);
    let log = logger.stop();
    assert!(
        log.keys
            .iter()
            .any(|k| k.code == 0x87 && k.down && k.mods & cam::project::recording::mods::CTRL != 0)
    );
    assert!(
        !log.keys
            .iter()
            .any(|k| matches!(k.code, 0x10..=0x12 | 0x5b | 0x5c | 0xa0..=0xa5))
    );
    let keys: Vec<_> = log
        .keys
        .iter()
        .map(|k| cam::project::recording::KeyEvent {
            t: k.host * 1000.0,
            code: k.code,
            key: k.key.clone(),
            mods: k.mods,
            down: k.down,
        })
        .collect();
    let badges = cam::engine::keys::badges(&keys, false);
    assert!(badges.iter().any(|b| b.label == "Ctrl+F24"), "{badges:?}");
    assert!(badges.iter().any(|b| b.label == "Enter"), "{badges:?}");
    assert!(!log.mouse.is_empty() && !log.cursors.is_empty());
    assert!(
        log.cursors
            .iter()
            .any(|c| c.file.as_ref().is_some_and(|file| dir.join(file).is_file()))
    );
    drop(window);
    println!("Native input artifacts: {}", dir.display());
}

fn quiet_tone(hz: f64) -> cpal::Stream {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let device = cpal::default_host().default_output_device().unwrap();
    let config = device.default_output_config().unwrap().config();
    let rate = config.sample_rate as f64;
    let channels = config.channels as usize;
    let mut frame = 0u64;
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [f32], _: &_| {
                for samples in out.chunks_mut(channels) {
                    samples.fill(
                        (std::f64::consts::TAU * hz * frame as f64 / rate).sin() as f32 * 0.04,
                    );
                    frame += 1;
                }
            },
            |e| eprintln!("test tone: {e}"),
            None,
        )
        .unwrap();
    stream.play().unwrap();
    stream
}

#[test]
#[ignore = "checks process-excluding loopback using two quiet speaker test tones"]
fn process_exclusion_keeps_external_audio() {
    if let Some(dir) = std::env::var_os("CAM_EXCLUSION_CHILD") {
        use cam::platform::AudioCaptureBackend;
        use std::sync::{Arc, Mutex};
        let dir = std::path::PathBuf::from(dir);
        let included = Arc::new(Mutex::new(Vec::new()));
        let excluded = Arc::new(Mutex::new(Vec::new()));
        let capture = |out: Arc<Mutex<Vec<[f32; 2]>>>| -> Box<dyn FnMut(&[f32], f64) + Send> {
            Box::new(move |samples, _| {
                out.lock()
                    .unwrap()
                    .extend(samples.chunks_exact(2).map(|s| [s[0], s[1]]))
            })
        };
        let backend = cam::platform::windows::WindowsAudioBackend::new();
        let mut all = backend
            .start_system_loopback(48000, 2, capture(included.clone()))
            .unwrap();
        let mut others = backend
            .start_loopback_excluding_self(capture(excluded.clone()))
            .unwrap();
        let recorder = Recorder::start(Options {
            target: Target::Area {
                display: 0,
                rect: Rect::new(100.0, 100.0, 320.0, 200.0),
            },
            dir: dir.clone(),
            name: Some("Recorder".into()),
            fps: 10,
            exclude_own_audio: true,
            ..Default::default()
        })
        .unwrap();
        let own = quiet_tone(1234.0);
        std::thread::sleep(Duration::from_millis(1200));
        drop(own);
        std::fs::write(dir.join("ready"), b"ready").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !dir.join("external.done").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "external tone timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(150));
        all.stop();
        others.stop();
        let package = cam::project::Package::new(recorder.stop().unwrap());
        let recorded = cam::media::read_audio(&package.rec("system.wav"), 0).unwrap();
        assert!(all.error().is_none(), "{:?}", all.error());
        assert!(others.error().is_none(), "{:?}", others.error());
        let included = included.lock().unwrap();
        let excluded = excluded.lock().unwrap();
        cam::app::player::write_wav(&dir.join("all.wav"), &included).unwrap();
        cam::app::player::write_wav(&dir.join("others.wav"), &excluded).unwrap();
        let own_all = cam::audiocheck::analyze(&included, 1234.0);
        let own_excluded = cam::audiocheck::analyze(&excluded, 1234.0);
        let external_all = cam::audiocheck::analyze(&included, 880.0);
        let external_excluded = cam::audiocheck::analyze(&excluded, 880.0);
        println!(
            "Own/all: {own_all:?}; own/excluded: {own_excluded:?}; external/all: {external_all:?}; external/excluded: {external_excluded:?}"
        );
        assert!(own_all.tone_seconds > 0.8, "{own_all:?}");
        assert!(own_excluded.tone_seconds < 0.1, "{own_excluded:?}");
        assert!(external_all.tone_seconds > 1.6, "{external_all:?}");
        assert!(
            external_excluded.tone_seconds > 1.6,
            "{external_excluded:?}"
        );
        let recorded_own = cam::audiocheck::analyze(&recorded, 1234.0);
        let recorded_external = cam::audiocheck::analyze(&recorded, 880.0);
        println!("Recorder/own: {recorded_own:?}; recorder/external: {recorded_external:?}");
        assert!(recorded_own.tone_seconds < 0.1, "{recorded_own:?}");
        assert!(
            recorded_external.tone_seconds > 1.6,
            "{recorded_external:?}"
        );
        return;
    }
    let dir = std::env::temp_dir().join(format!(
        "cam-exclusion-{}-{}",
        std::process::id(),
        cam::util::host_now().to_bits()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // The capture worker is a child of the test runner. Audio from this
    // parent is outside that child's excluded process tree.
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
    let mut child = command
        .args([
            "--ignored",
            "--exact",
            "process_exclusion_keeps_external_audio",
            "--nocapture",
        ])
        .env("CAM_EXCLUSION_CHILD", &dir)
        .stdout(std::fs::File::create(dir.join("child.log")).unwrap())
        .stderr(std::fs::File::create(dir.join("child-errors.log")).unwrap())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    while !dir.join("ready").exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "capture child failed: {}",
            dir.display()
        );
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("capture child timed out: {}", dir.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let external = quiet_tone(880.0);
    std::thread::sleep(Duration::from_millis(2200));
    drop(external);
    std::fs::write(dir.join("external.done"), b"done").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            println!(
                "{}",
                std::fs::read_to_string(dir.join("child.log")).unwrap()
            );
            assert!(status.success(), "capture child failed: {}", dir.display());
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("capture child did not stop: {}", dir.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("Process-exclusion artifacts: {}", dir.display());
}
