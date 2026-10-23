# Cam 0.1.4

This release keeps the v0.1.2 rendering, decoding, capture, and audio paths after
the v0.1.3 rollback requested for performance.

- Launch the Windows GUI without opening a terminal window.
- Let command-line commands use an existing terminal without allocating a new
  one; keep redirected output and detached recording/export working.
- Update installer acceptance to wait for GUI-subsystem CLI commands and check
  their actual exit codes.
- Download setup media tools from the publisher's GitHub mirror with the same
  pinned archive version and SHA-256 checksum.

The reverted v0.1.3 GPU and audio changes are not included. This release does not
claim to fix unrelated issues inherited from v0.1.2.

## Windows downloads

Use **Cam-0.1.4-Windows-x64-Setup.exe**, or extract
**Cam-0.1.4-Windows-x64-Setup.zip** and run `Install.cmd`. Setup downloads verified
FFmpeg/ffprobe tools and retains existing recordings and settings. Close Cam
before updating.

The standalone `cam.exe` is also provided for users with the media tools and
runtime dependencies already installed. Checksums and license files accompany
the downloads.

## Verification

The optimized build and console-control regression passed. Three alternating
comparisons on the same 1080p recording measured 164 median preview frames/s
for both v0.1.2 and v0.1.4, with identical final-frame error (MSE 3.683). Median
seek settling was 61 ms and 56 ms respectively. These measurements apply to
the tested PC and recording.
