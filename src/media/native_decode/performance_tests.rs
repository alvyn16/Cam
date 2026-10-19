use super::*;
use windows62::Win32::{Graphics::Dxgi::*, System::Threading::*};

fn cpu_seconds() -> f64 {
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .unwrap()
    };
    let ticks = |t: windows62::Win32::Foundation::FILETIME| {
        ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
    };
    (ticks(kernel) + ticks(user)) as f64 / 10_000_000.0
}

#[test]
#[ignore = "profiles actual 4K GPU preview conversion/readback and persistent decoder playback"]
fn high_resolution_preview_throughput_and_resources() {
    let path = std::path::PathBuf::from(
        std::env::var_os("CAM_TEST_VIDEO").expect("set CAM_TEST_VIDEO to a 4K source"),
    );
    let video = super::super::VideoReader::open(&path).unwrap();
    let track = Track {
        path,
        width: video.size.0,
        height: video.size.1,
        fps: video.fps,
        duration_ms: video.duration_ms,
    };
    let mut reader = Reader::open(&track).unwrap();
    for (name, media) in [
        ("native", unsafe {
            reader
                .reader
                .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0)
                .unwrap()
        }),
        ("current", unsafe {
            reader
                .reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .unwrap()
        }),
    ] {
        eprintln!(
            "{name}: matrix={:?}, range={:?}",
            unsafe { media.GetUINT32(&MF_MT_YUV_MATRIX) },
            unsafe { media.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE) }
        );
    }
    let first = reader.frame_at(0.0, &|| false).unwrap().unwrap();
    drop(first);
    // Isolate conversion/readback from compressed video decode. Use a real
    // decoder sample, including its padded dimensions and array slice.
    reader.completion.begin();
    unsafe {
        reader
            .reader
            .ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                None,
                None,
                None,
            )
            .unwrap()
    };
    let sample = reader
        .completion
        .wait(&|| false, std::time::Duration::from_secs(5))
        .unwrap()
        .sample
        .unwrap();
    for _ in 0..8 {
        drop(reader.read_image(&sample, 0.0).unwrap());
    }
    let cpu = cpu_seconds();
    let started = std::time::Instant::now();
    for _ in 0..240 {
        let image = reader.read_image(&sample, 0.0).unwrap();
        assert_eq!(image.size(), reader.size);
        assert_eq!(image.bgra[3], 255);
    }
    let conversion_wall = started.elapsed().as_secs_f64();
    let conversion_cpu = cpu_seconds() - cpu;
    let cpu = cpu_seconds();
    let started = std::time::Instant::now();
    let playback_frames = ((track.duration_ms * track.fps / 1000.0).floor() as u32)
        .saturating_sub(2)
        .min(240);
    assert!(playback_frames > 0);
    for i in 1..=playback_frames {
        let at = i as f64 * 1000.0 / track.fps;
        let frame = reader.frame_at(at, &|| false).unwrap().unwrap();
        assert!((frame.pts_ms - at).abs() <= 1000.0 / track.fps + 1.0);
    }
    let playback_wall = started.elapsed().as_secs_f64();
    let playback_cpu = cpu_seconds() - cpu;
    let buffer = unsafe { sample.GetBufferByIndex(0).unwrap() };
    let dxgi: IMFDXGIBuffer = buffer.cast().unwrap();
    let mut raw = std::ptr::null_mut();
    unsafe { dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw).unwrap() };
    let texture = unsafe { ID3D11Texture2D::from_raw(raw) };
    let device = unsafe { texture.GetDevice().unwrap() };
    let adapter: IDXGIAdapter3 =
        unsafe { device.cast::<IDXGIDevice>().unwrap().GetAdapter().unwrap() }
            .cast()
            .unwrap();
    let mut memory = Default::default();
    unsafe {
        adapter
            .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut memory)
            .unwrap()
    };
    let metrics = serde_json::json!({"frames":240,"playback_frames":playback_frames,"source":[track.width,track.height],"conversion_seconds":conversion_wall,"conversion_cpu_seconds":conversion_cpu,"conversion_fps":240.0/conversion_wall,"playback_seconds":playback_wall,"playback_cpu_seconds":playback_cpu,"playback_fps":playback_frames as f64/playback_wall,"local_gpu_bytes":memory.CurrentUsage});
    eprintln!("Preview performance: {metrics}");
    if let Some(output) = std::env::var_os("CAM_PERF_RESULT") {
        std::fs::write(output, metrics.to_string()).unwrap();
    }
}
