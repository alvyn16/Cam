# Additional runtime and dependency notices

Cam is derived from Michael Shimeles's Glide and retains the repository's MIT/Apache-2.0 notices. Inter's license is in assets/fonts/OFL.txt.

- windows-capture 2.0.1: MIT, https://github.com/NiiightmareXD/windows-capture. Supplies the Windows Graphics Capture integration. Uses Windows graphics APIs and does not change Cam's project format.
- nnnoiseless 0.5.2: BSD-3-Clause, https://github.com/jneem/nnnoiseless. Supplies RNNoise-based neural denoising and its built-in model. The crate retains its upstream copyright/license notices in Cargo's dependency source and distributed dependency licensing must preserve them.
- FFmpeg and ffprobe are external executables: https://ffmpeg.org/legal.html. They are not bundled by these build scripts. Their license depends on the build's enabled components; anyone distributing them with Cam must include the corresponding notices and satisfy that build's license/source obligations.
