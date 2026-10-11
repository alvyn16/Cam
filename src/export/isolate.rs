//! Voice isolation: run a microphone track through Apple's on-device ML
//! `AUSoundIsolation` audio unit offline (AudioToolbox C API, pull model).
//! Removes background noise far better than a gate; returns None when the
//! unit isn't available so callers can fall back to simple DSP.

use std::ffi::c_void;

use super::audio::Stereo;
use crate::capture::mixer::RATE;

#[repr(C)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

#[repr(C)]
#[derive(Default)]
struct SmpteTime {
    subframes: i16,
    subframe_divisor: i16,
    counter: u32,
    kind: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
#[derive(Default)]
struct AudioTimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock_time: u64,
    smpte_time: SmpteTime,
    flags: u32,
    reserved: u32,
}

type RenderCallback = extern "C" fn(*mut c_void, *mut u32, *const AudioTimeStamp, u32, u32, *mut AudioBufferList) -> i32;

#[repr(C)]
struct AuRenderCallbackStruct {
    input_proc: RenderCallback,
    input_proc_ref_con: *mut c_void,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(component: *mut c_void, desc: *const AudioComponentDescription) -> *mut c_void;
    fn AudioComponentInstanceNew(component: *mut c_void, out: *mut *mut c_void) -> i32;
    fn AudioComponentInstanceDispose(unit: *mut c_void) -> i32;
    fn AudioUnitSetProperty(unit: *mut c_void, id: u32, scope: u32, element: u32, data: *const c_void, size: u32) -> i32;
    fn AudioUnitGetProperty(unit: *mut c_void, id: u32, scope: u32, element: u32, data: *mut c_void, size: *mut u32) -> i32;
    fn AudioUnitInitialize(unit: *mut c_void) -> i32;
    fn AudioUnitUninitialize(unit: *mut c_void) -> i32;
    fn AudioUnitRender(unit: *mut c_void, flags: *mut u32, ts: *const AudioTimeStamp, bus: u32, frames: u32, data: *mut AudioBufferList) -> i32;
}

const fn fourcc(b: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*b)
}
const PROP_STREAM_FORMAT: u32 = 8;
const PROP_MAX_FRAMES: u32 = 14;
const PROP_LATENCY: u32 = 12;
const PROP_RENDER_CALLBACK: u32 = 23;
const SCOPE_GLOBAL: u32 = 0;
const SCOPE_INPUT: u32 = 1;
const SCOPE_OUTPUT: u32 = 2;
const FLOAT_PACKED_NONINTERLEAVED: u32 = 1 | 8 | 32;
const SLICE: u32 = 512;

/// Source samples for the render callback.
struct Feed {
    mono: Vec<f32>,
    pos: usize,
}

extern "C" fn feed(
    ref_con: *mut c_void,
    _flags: *mut u32,
    _ts: *const AudioTimeStamp,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> i32 {
    // SAFETY: ref_con is the Feed we registered; data has one buffer of
    // `frames` f32 samples (mono, non-interleaved) owned by the unit.
    unsafe {
        let f = &mut *(ref_con as *mut Feed);
        let buf = &mut (*data).buffers[0];
        let out = std::slice::from_raw_parts_mut(buf.data as *mut f32, frames as usize);
        for o in out.iter_mut() {
            *o = f.mono.get(f.pos).copied().unwrap_or(0.0);
            f.pos += 1;
        }
    }
    0
}

/// Isolate speech in a 48 kHz stereo track (processed as mono). None if the
/// system has no AUSoundIsolation or it fails to run.
pub fn isolate_voice(input: &[Stereo]) -> Option<Vec<Stereo>> {
    if input.is_empty() {
        return Some(vec![]);
    }
    let desc = AudioComponentDescription {
        component_type: fourcc(b"aufx"),
        component_sub_type: fourcc(b"vois"),
        component_manufacturer: fourcc(b"appl"),
        component_flags: 0,
        component_flags_mask: 0,
    };
    // SAFETY: standard AudioUnit lifecycle; every pointer passed lives for
    // the duration of the call (the Feed outlives all renders).
    unsafe {
        let comp = AudioComponentFindNext(std::ptr::null_mut(), &desc);
        if comp.is_null() {
            return None;
        }
        let mut unit = std::ptr::null_mut();
        if AudioComponentInstanceNew(comp, &mut unit) != 0 || unit.is_null() {
            return None;
        }
        let result = run(unit, input);
        AudioUnitUninitialize(unit);
        AudioComponentInstanceDispose(unit);
        result
    }
}

unsafe fn run(unit: *mut c_void, input: &[Stereo]) -> Option<Vec<Stereo>> {
    unsafe {
        let asbd = AudioStreamBasicDescription {
            sample_rate: RATE,
            format_id: fourcc(b"lpcm"),
            format_flags: FLOAT_PACKED_NONINTERLEAVED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 1,
            bits_per_channel: 32,
            reserved: 0,
        };
        let size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
        let p = &asbd as *const _ as *const c_void;
        if AudioUnitSetProperty(unit, PROP_STREAM_FORMAT, SCOPE_INPUT, 0, p, size) != 0
            || AudioUnitSetProperty(unit, PROP_STREAM_FORMAT, SCOPE_OUTPUT, 0, p, size) != 0
        {
            return None;
        }
        let max = SLICE;
        AudioUnitSetProperty(unit, PROP_MAX_FRAMES, SCOPE_GLOBAL, 0, &max as *const u32 as *const c_void, 4);
        let mut source = Feed { mono: input.iter().map(|s| (s[0] + s[1]) * 0.5).collect(), pos: 0 };
        let cb = AuRenderCallbackStruct { input_proc: feed, input_proc_ref_con: &mut source as *mut Feed as *mut c_void };
        if AudioUnitSetProperty(
            unit,
            PROP_RENDER_CALLBACK,
            SCOPE_INPUT,
            0,
            &cb as *const _ as *const c_void,
            std::mem::size_of::<AuRenderCallbackStruct>() as u32,
        ) != 0
        {
            return None;
        }
        if AudioUnitInitialize(unit) != 0 {
            return None;
        }
        let mut latency = 0f64;
        let mut lsize = 8u32;
        AudioUnitGetProperty(unit, PROP_LATENCY, SCOPE_GLOBAL, 0, &mut latency as *mut f64 as *mut c_void, &mut lsize);
        let skip = (latency.max(0.0) * RATE).round() as usize;
        let want = input.len() + skip;
        let mut out: Vec<f32> = Vec::with_capacity(want + SLICE as usize);
        let mut chunk = vec![0f32; SLICE as usize];
        let mut ts = AudioTimeStamp { flags: 1, ..Default::default() };
        while out.len() < want {
            let mut list = AudioBufferList {
                number_buffers: 1,
                buffers: [AudioBuffer { number_channels: 1, data_byte_size: SLICE * 4, data: chunk.as_mut_ptr() as *mut c_void }],
            };
            let mut flags = 0u32;
            if AudioUnitRender(unit, &mut flags, &ts, 0, SLICE, &mut list) != 0 {
                return None;
            }
            out.extend_from_slice(&chunk);
            ts.sample_time += SLICE as f64;
        }
        Some(out[skip..skip + input.len()].iter().map(|&v| [v, v]).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolation_keeps_length_and_reduces_steady_noise() {
        // Hiss alone: voice isolation should remove most of it.
        let mut x = 12345u32;
        let noise: Vec<Stereo> = (0..48_000 * 2)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let v = (x as f32 / u32::MAX as f32 - 0.5) * 0.1;
                [v, v]
            })
            .collect();
        let Some(out) = isolate_voice(&noise) else {
            eprintln!("AUSoundIsolation unavailable; skipping");
            return;
        };
        assert_eq!(out.len(), noise.len());
        let rms = |b: &[Stereo]| (b.iter().map(|s| s[0] * s[0]).sum::<f32>() / b.len() as f32).sqrt();
        let (before, after) = (rms(&noise[48_000..]), rms(&out[48_000..]));
        assert!(after < before * 0.5, "noise rms {before} -> {after}");
    }
}
