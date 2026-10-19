use super::NativeWriter;
use crate::{
    media::writer::{MovieWriter, WriterSpec},
    project::model::Codec,
};
use windows62::Win32::{
    Foundation::HMODULE,
    Graphics::{Direct3D::D3D_DRIVER_TYPE_HARDWARE, Direct3D11::*, Dxgi::Common::*},
};

#[test]
#[ignore = "compares actual 4K hardware encoding from GPU textures and raw CPU pixels"]
fn direct_capture_4k_encoding_preserves_frames_colors_and_throughput() {
    let root = std::env::temp_dir().join(format!(
        "cam-capture-4k-{}-{}",
        std::process::id(),
        crate::util::host_now().to_bits()
    ));
    std::fs::create_dir(&root).unwrap();
    let spec = WriterSpec {
        width: 3840,
        height: 2160,
        fps: 60,
        codec: Codec::Hevc,
        bitrate: 32_000_000,
        quality: Some(0.95),
        audio: false,
    };
    let (mut device, mut context) = (None, None);
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .unwrap();
    }
    let device = device.unwrap();
    let context = context.unwrap();
    let mut texture = None;
    unsafe {
        device
            .CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: spec.width,
                    Height: spec.height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut texture),
            )
            .unwrap();
    }
    let texture = texture.unwrap();
    let mut view = None;
    unsafe {
        device
            .CreateRenderTargetView(&texture, None, Some(&mut view))
            .unwrap();
    }
    let view = view.unwrap();
    let output = root.join("native.mov");
    let mut writer = NativeWriter::create_for_capture(&output, &spec, device.clone()).unwrap();
    let started = std::time::Instant::now();
    for i in 0..120 {
        unsafe {
            context.ClearRenderTargetView(
                &view,
                if i < 60 {
                    &[0.0, 0.0, 1.0, 1.0]
                } else {
                    &[1.0, 0.0, 0.0, 1.0]
                },
            );
        }
        writer.push_texture(&texture, &|| false).unwrap();
    }
    writer.finish().unwrap();
    let native_seconds = started.elapsed().as_secs_f64();
    let blue = [255, 0, 0, 255].repeat((spec.width * spec.height) as usize);
    let red = [0, 0, 255, 255].repeat((spec.width * spec.height) as usize);
    let raw_output = root.join("cpu.mov");
    let mut writer = MovieWriter::create(&raw_output, &spec).unwrap();
    let buffer = writer.buffer().unwrap();
    let started = std::time::Instant::now();
    for i in 0..120 {
        if i == 0 {
            *buffer.bgra.lock().unwrap() = blue.clone();
        }
        if i == 60 {
            *buffer.bgra.lock().unwrap() = red.clone();
        }
        writer.push_frame(&buffer).unwrap();
    }
    writer.finish().unwrap();
    let cpu_seconds = started.elapsed().as_secs_f64();
    for path in [&output, &raw_output] {
        let probe = crate::media::ffmpeg::probe(path).unwrap();
        assert_eq!(probe["streams"][0]["width"], 3840);
        assert_eq!(probe["streams"][0]["height"], 2160);
        assert_eq!(probe["streams"][0]["codec_name"], "hevc");
        let decoded = crate::media::ffmpeg::output(
            crate::media::ffmpeg::command("ffmpeg")
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(path)
                .args([
                    "-an",
                    "-vf",
                    "scale=1:1",
                    "-pix_fmt",
                    "rgb24",
                    "-f",
                    "rawvideo",
                    "pipe:1",
                ]),
        )
        .unwrap();
        assert_eq!(decoded.stdout.len(), 120 * 3);
        let blue = &decoded.stdout[..3];
        let red = &decoded.stdout[119 * 3..120 * 3];
        assert!(blue[2] > 220 && blue[0] < 25, "blue changed: {blue:?}");
        assert!(red[0] > 220 && red[2] < 25, "red changed: {red:?}");
    }
    let metrics = serde_json::json!({"frames":120,"size":[3840,2160],"native_seconds":native_seconds,"native_fps":120.0/native_seconds,"cpu_raw_seconds":cpu_seconds,"cpu_raw_fps":120.0/cpu_seconds,"artifacts":root});
    std::fs::write(
        root.join("result.json"),
        serde_json::to_vec_pretty(&metrics).unwrap(),
    )
    .unwrap();
    eprintln!("4K capture encoder: {metrics}");
}
