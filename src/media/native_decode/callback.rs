//! One outstanding asynchronous MF read. Callbacks only retain a sample and
//! signal; seek cancellation flushes pending reads before reusing the decoder.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::c_void,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use windows62::{
    Win32::{
        Foundation::{E_NOINTERFACE, E_POINTER, S_OK},
        Media::MediaFoundation::{
            IMFSample, IMFSourceReaderCallback, IMFSourceReaderCallback_Vtbl,
        },
    },
    core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface},
};
pub(super) struct Read {
    pub status: HRESULT,
    pub flags: u32,
    pub pts: i64,
    pub sample: Option<IMFSample>,
}
// MF samples delivered by Source Reader can be passed to processing threads;
// D3D11 contexts using these samples are multithread protected by Reader.
unsafe impl Send for Read {}
#[derive(Default)]
struct State {
    read: Option<Read>,
    pending: bool,
    flushing: bool,
    flushed: bool,
}
#[derive(Default)]
pub(super) struct Completion {
    state: Mutex<State>,
    changed: Condvar,
}
impl Completion {
    pub fn begin(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.read = None;
        state.pending = true;
    }
    pub fn wait(&self, cancelled: &impl Fn() -> bool, timeout: Duration) -> Result<Read> {
        let until = Instant::now() + timeout;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            ensure!(!cancelled(), "native preview seek superseded");
            if let Some(read) = state.read.take() {
                read.status.ok().context("reading native preview")?;
                return Ok(read);
            }
            ensure!(Instant::now() < until, "native preview read timed out");
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(5))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
    pub fn begin_flush(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.flushing = true;
        state.flushed = false;
        state.read = None;
    }
    pub fn wait_flush(&self) -> bool {
        let until = Instant::now() + Duration::from_secs(1);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !state.flushed {
            if Instant::now() >= until {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(5))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }
}
#[repr(C)]
struct Callback {
    vtable: *const IMFSourceReaderCallback_Vtbl,
    references: AtomicU32,
    completion: Arc<Completion>,
}
const AGILE: GUID = GUID::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);
unsafe extern "system" fn query(
    this: *mut c_void,
    iid: *const GUID,
    output: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || output.is_null() {
        return E_POINTER;
    }
    unsafe {
        *output = std::ptr::null_mut();
        let iid = *iid;
        if iid != IUnknown::IID && iid != IMFSourceReaderCallback::IID && iid != AGILE {
            return E_NOINTERFACE;
        }
        *output = this;
        add_ref(this);
    }
    S_OK
}
unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe { &*this.cast::<Callback>() }
        .references
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let remaining = unsafe { &*this.cast::<Callback>() }
        .references
        .fetch_sub(1, Ordering::Release)
        - 1;
    if remaining == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(this.cast::<Callback>()));
        }
    }
    remaining
}
unsafe extern "system" fn read_sample(
    this: *mut c_void,
    status: HRESULT,
    _stream: u32,
    flags: u32,
    pts: i64,
    sample: *mut c_void,
) -> HRESULT {
    let callback = unsafe { &*this.cast::<Callback>() };
    let sample = if status.is_ok() {
        unsafe { IMFSample::from_raw_borrowed(&sample) }.cloned()
    } else {
        None
    };
    let mut state = callback
        .completion
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if state.pending && !state.flushing {
        state.read = Some(Read {
            status,
            flags,
            pts,
            sample,
        });
        state.pending = false;
    }
    callback.completion.changed.notify_all();
    S_OK
}
unsafe extern "system" fn flush(this: *mut c_void, _stream: u32) -> HRESULT {
    let callback = unsafe { &*this.cast::<Callback>() };
    let mut state = callback
        .completion
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    state.read = None;
    state.pending = false;
    state.flushing = false;
    state.flushed = true;
    callback.completion.changed.notify_all();
    S_OK
}
unsafe extern "system" fn event(_: *mut c_void, _: u32, _: *mut c_void) -> HRESULT {
    S_OK
}
static VTABLE: IMFSourceReaderCallback_Vtbl = IMFSourceReaderCallback_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: query,
        AddRef: add_ref,
        Release: release,
    },
    OnReadSample: read_sample,
    OnFlush: flush,
    OnEvent: event,
};
pub(super) fn callback() -> (IMFSourceReaderCallback, Arc<Completion>) {
    let completion = Arc::new(Completion::default());
    let callback = Box::new(Callback {
        vtable: &VTABLE,
        references: AtomicU32::new(1),
        completion: completion.clone(),
    });
    // repr(C) places the static COM vtable first; the initial owned reference
    // transfers to IMFSourceReaderCallback and is freed by Release.
    (
        unsafe { IMFSourceReaderCallback::from_raw(Box::into_raw(callback).cast()) },
        completion,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_flushes_cancellation_and_callback_lifetime_are_bounded() {
        let (interface, completion) = callback();
        let unknown = interface.cast::<IUnknown>().unwrap();
        drop(unknown);
        completion.begin();
        unsafe {
            let _ = read_sample(interface.as_raw(), S_OK, 0, 8, 1234, std::ptr::null_mut());
        }
        let read = completion.wait(&|| false, Duration::from_secs(1)).unwrap();
        assert_eq!((read.flags, read.pts), (8, 1234));
        completion.begin();
        completion.begin_flush();
        unsafe {
            let _ = read_sample(interface.as_raw(), S_OK, 0, 1, 999, std::ptr::null_mut());
            let _ = flush(interface.as_raw(), 0);
        }
        assert!(completion.wait_flush());
        assert!(completion.state.lock().unwrap().read.is_none());
        completion.begin();
        let started = Instant::now();
        assert!(completion.wait(&|| true, Duration::from_secs(1)).is_err());
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(
            completion
                .wait(&|| false, Duration::from_millis(2))
                .is_err()
        );
        completion.begin();
        unsafe {
            let _ = read_sample(
                interface.as_raw(),
                windows62::Win32::Foundation::E_FAIL,
                0,
                0,
                0,
                1usize as *mut c_void,
            );
        }
        assert!(completion.wait(&|| false, Duration::from_secs(1)).is_err());
        drop(interface);
        assert_eq!(Arc::strong_count(&completion), 1);
    }
}
