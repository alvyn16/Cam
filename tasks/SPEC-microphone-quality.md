# Microphone recording quality

Report: speech is distorted and difficult to understand in the raw recording, editor and export; game/system audio is usable.

Confirmed in untouched user recordings: the 35.069-second clip from 18.23.42 contains 1,552 internal zero runs of at least 0.5 ms, totaling 7.915 seconds. The first 60 seconds of the 16.52.25 microphone contain 2,324 such runs totaling 27.067 seconds. Neither corresponding system track contains those internal runs. Microphone channels are identical and are not clipping, ruling out stereo phase cancellation and input overload for those files.

The WASAPI backend discarded CPAL's hardware capture timestamp, placed each packet at callback-arrival time minus its duration, and restarted a linear resampler for each packet. A callback that drains queued packets overwrites previous samples and leaves holes. The default Realtek ring held only 10 ms, making real overruns under scheduling pressure more likely too.

Zero-run counts alone do not distinguish device-supplied digital silence from recorder-inserted gaps. The later direct sample comparison confirmed that remaining digital silence was also present in the microphone feed, with the saved samples matching that feed. A startup discontinuity was caught separately; capture now requires 500 ms of continuous device samples before acknowledging microphone startup. Narration starts its saved clock after initialization so setup time neither shifts its placement nor truncates its tail.

Requirements:
- Place samples using native WASAPI/QPC timestamps mapped to the video clock. Packet arrival jitter must not change placement.
- Preserve resampling phase/history and fractional frame counts across packets; bound filter history and suppress aliasing when downsampling. Native 48 kHz samples remain unchanged.
- Provide a 100-ms shared-mode capture ring without changing callback cadence. Keep blocking diagnostics off the device thread.
- Dispatch captured packets to a separate conversion/consumer worker with an 8 MiB byte budget and a bounded packet count. Slow consumers must not block WASAPI; overload must be reported, and stopping must drain queued audio and join the worker after the native stream closes.
- Wait for real steady microphone data before starting the recording clock; quiet samples must still count as valid input. Loopback must remain usable when speakers are idle.
- Preserve true device discontinuities and pause-adjusted timing; never invent replacement speech or silently edit existing recordings.
- Use normalized conversion for signed, unsigned and floating-point input formats; preserve microphone mono duplication and system stereo.
- Propagate voice-over capture/disk errors and preserve recoverable audio.
- Verify deterministic packet/format/frequency tests, actual microphone timing with delayed callbacks, and capture through editor mixing, enhancement, playback and independently decoded exports.

Existing recordings are preserved. These fixes apply to new capture; they cannot reconstruct any words that are already missing from source media. The zero-run measurements do not establish how many words, if any, were lost by the old capture path.

Intermediate acceptance: packet-independent resampling, alias rejection, exact native-rate samples, bounded dispatch, overload reporting and drain/join tests passed. The 25-ms delayed-consumer live microphone test delivered 601 packets (288,480 frames), with zero gaps or overlaps and 16.134 ms maximum consumer-arrival jitter. Earlier single-threaded attempts exposed real device overruns and were not accepted. Final release identity and end-to-end checks are recorded in IMPLEMENTATION_STATUS.md.

Final acceptance: 156 standard tests and three explicit hardware tests passed. A direct source-versus-saved comparison during screen recording preserved microphone samples with zero device-clock gaps/overlaps. The final release retained all 25 known speech-control words through both raw and enhanced exports. The user then listened to a fresh recording of their own voice from the built-in Realtek microphone and confirmed it was clear and easy to understand; that clip's enhanced export independently decoded successfully. See IMPLEMENTATION_STATUS.md for the release hash and private temporary artifact locations.
