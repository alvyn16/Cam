# Camera preview while recording

Add an opt-in switch directly below the camera device controls in recording setup. Reuse the existing label/switch rows, spacing, typography and disabled states. Persist the preference; older settings default to off. Camera-off disables the switch without forgetting the preference.

While recording with Camera and Show while recording enabled, show a small always-on-top camera monitor. Reuse the current recording camera session and one replaceable frame; do not open the webcam twice. Respect the camera's mirror and shape preferences. Match Cam's palette, Inter typography, rounded panels and existing buttons. Drag the image to move the monitor; hide closes only the monitor. The camera continues recording independently. Pausing keeps the monitor live.

Exclude the monitor from screen capture, skip the taskbar and avoid taking keyboard focus. Stop/cancel/restart/error/quit must release the window and preview texture. No camera-monitor UI is included in project styling or baked into screen footage; the independently recorded camera track remains editable.

Verify settings compatibility, real recorder frame delivery during recording and pause, both toggle states, window exclusion/nonactivation and teardown, rendered setup/monitor UI, standard tests and an optimized real GUI recording/export.
