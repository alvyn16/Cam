# Progress

Working name: **Glide** (`glide`). Milestones track the parity checklist in
`analysis/SCREEN_STUDIO.md`.

| # | Milestone | Status |
|---|---|---|
| M0 | Reverse engineering with REA, parity checklist | ☑ `analysis/SCREEN_STUDIO.md` |
| M1 | Engine: springs, deterministic animator, timeline map, sizes, auto zoom + targeting, cursor model, click effects, camera layouts, shortcut badges, captions | ☑ 37 unit tests |
| M2 | GPU renderer (wgpu/Metal): background, shadow, rounded window + inset, cursor, click FX, motion blur, camera, text | ☐ |
| M3 | Media: AVAssetReader decode with seeking | ☐ |
| M4 | Recording: ScreenCaptureKit display/window/area, system audio, mic, camera, input + cursor logging | ☐ |
| M5 | Export: MP4 (VideoToolbox H.264/HEVC), audio mix with cuts/speed, GIF | ☐ |
| M6 | Editor UI (egui + Inter): preview, sidebar sections, timeline | ☐ |
| M7 | Captions (on-device speech), shortcut overlay text | ☐ |
| M8 | App bundle, icon, signing | ☐ |
| M9 | Verification with real recordings; PERFORMANCE.md | ☐ |

## Log

- 2026-10-11: REA analysis of Screen Studio 3.7.1; engine core with tests.
