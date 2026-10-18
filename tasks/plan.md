# Complete the Cam Windows implementation

Implement the findings in GLIDE_COMPARISON.md in verifiable slices. Preserve existing user edits. Use real Windows capture and FFmpeg media interchange; fail clearly when a requested device or external runtime is unavailable. Do not describe hardware-dependent paths as verified without hardware evidence.

Order: media interoperability → capture/geometry/preview → audio/camera → native controls/input/integration → end-to-end verification/documentation.

Each item in todo.md requires a compiling build, focused regression coverage, and the stated live check where hardware permits. Final acceptance is real moving screen content with separate audio through recording, editing and independently decoded export; existing synthetic tests alone are insufficient.

Risks: absent webcam/iPhone; GPU/encoder differences; Windows privacy restrictions; third-party FFmpeg availability. Handle these explicitly and document evidence. Original upstream GPU zero-copy optimization remains a distinct performance requirement, not interchangeable with a CPU fallback.
