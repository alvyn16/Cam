# Performance

All numbers are real measurements on this MacBook Pro (Apple M4 Pro, 24 GB,
macOS 27.0.1), release build, from `scripts/bench.sh` (2026-10-11). Re-run it
after performance work and update this file.

## Recording overhead

`glide record -t 10`, measured with `/usr/bin/time -l` over the whole recorder
process (ScreenCaptureKit capture, hardware HEVC encode, 120 Hz mouse/cursor
logging, package write). CPU is user + system time divided by wall time.

| Recording | Wall | CPU time | CPU load | Peak memory |
|---|---|---|---|---|
| 5K display (5120×2880 @ 60 fps, HEVC), 10 s | 10.7 s | 1.07 s | **10 % of one core** | **56 MB** |
| Generated test pattern (1080p @ 60 fps), 10 s | 11.5 s | 2.41 s | 21 % of one core (pattern drawing on the CPU) | 91 MB |

A 10 s 5K60 recording is 16 MB on disk (~13 Mbit/s HEVC). The encoder runs on
the media engine, so recording leaves the CPU and GPU almost idle for the app
being recorded. The full GUI flow (launch, record 3 s, pause/resume, stop,
open editor, export, quit) peaks at 172 MB resident.

## Export speed

"× real time" = seconds of output video per second of wall-clock export.
Export renders every frame on the GPU (wgpu/Metal) directly into the
encoder's pixel buffers (zero-copy via `CVMetalTextureCache`), with decode,
render and encode pipelined.

| Source | Output | Time | Speed |
|---|---|---|---|
| Synthetic 2560×1600 HEVC, 12 s | 1594×1080 @ 60, H.264 "social" | 2.84 s | **4.2×** |
| Synthetic 2560×1600 HEVC, 12 s | 1594×1080 @ 60, HEVC | 2.84 s | 4.2× |
| Synthetic 2560×1600 HEVC, 12 s | 1062×720 @ 30, H.264 "web" | 1.45 s | 8.3× |
| Synthetic 2560×1600 HEVC, 12 s | 3188×2160 @ 60, H.264 "studio" | 9.89 s | 1.2× |
| Synthetic 2560×1600 HEVC, 12 s | 1062×720 GIF @ 15 | 1.60 s | 7.5× |
| Real 5K display recording, 10.1 s | 1920×1080 @ 60, H.264 | 4.05 s | **2.5×** |
| Real 5K display recording, 10.1 s | 3840×2160 @ 60, H.264 "studio" | 10.07 s | 1.0× |
| Real 5K display recording, 10.1 s | 1280×720 GIF @ 15 | 3.34 s | 3.0× |

### Where the time goes (`GLIDE_PROFILE=1`)

| Export | Decode + upload | Render + submit | Encoder back-pressure |
|---|---|---|---|
| 5K recording → 1080p60 | 2.41 s | 1.13 s | 0.11 s |
| Synthetic → 4K60 | 0.04 s | 2.23 s | 7.06 s |

- **1080p from a 5K source is decode-bound**: decoding 605 frames of
  5120×2880 HEVC is the largest stage. Rendering a 1080p frame (background,
  shadow, rounded window, motion blur, cursor, effects) takes under 2 ms.
- **4K is encoder-bound**: the media engine encodes ~100 frames/s at
  3188×2160 (≈690 Mpixel/s), with rendering at ~320 frames/s. Asking the
  encoder to prioritise speed over quality
  (`PrioritizeEncodingSpeedOverQuality`) made no difference, so this is the
  hardware limit for a single encode engine.
- The GPU renderer has ample headroom for live preview: 535 frames/s at
  1080p and 320 frames/s at 4K in the export pipeline, versus 60 needed.

## How to reproduce

```sh
scripts/bench.sh            # needs Screen Recording permission for the terminal
scripts/bench.sh --no-screen  # synthetic sources only
GLIDE_PROFILE=1 dist/glide export Project.glide --height 2160
```
