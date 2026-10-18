//! Thread-safe, reference-counted COM completion callback. Manual vtables keep
//! this interface on windows-core 0.62 without mixing the existing 0.58 macros.
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
        Media::MediaFoundation::{IMFSinkWriterCallback, IMFSinkWriterCallback_Vtbl},
    },
    core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface},
};

#[derive(Default)]
pub(super) struct Completion {
    result: Mutex<Option<HRESULT>>,
    changed: Condvar,
}
impl Completion {
    pub fn wait(&self, cancelled: &dyn Fn() -> bool, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut result = self.result.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            ensure!(!cancelled(), "native encoding cancelled");
            if let Some(status) = *result {
                return status.ok().context("finalizing native GPU video");
            }
            ensure!(
                Instant::now() < deadline,
                "native encoder finalization timed out"
            );
            result = self
                .changed
                .wait_timeout(result, Duration::from_millis(10))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[repr(C)]
struct Callback {
    vtable: *const IMFSinkWriterCallback_Vtbl,
    references: AtomicU32,
    completion: Arc<Completion>,
}
// IAgileObject is a marker with IUnknown's vtable. This callback has no
// apartment-bound state: only atomics and an Arc containing a mutex/condvar.
const AGILE_IID: GUID = GUID::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);

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
    }
    let iid = unsafe { *iid };
    if iid != IUnknown::IID && iid != IMFSinkWriterCallback::IID && iid != AGILE_IID {
        return E_NOINTERFACE;
    }
    unsafe {
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
    let count = unsafe { &*this.cast::<Callback>() }
        .references
        .fetch_sub(1, Ordering::Release)
        - 1;
    if count == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(this.cast::<Callback>()));
        }
    }
    count
}
unsafe extern "system" fn finalized(this: *mut c_void, status: HRESULT) -> HRESULT {
    let callback = unsafe { &*this.cast::<Callback>() };
    let mut result = callback
        .completion
        .result
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if result.is_none() {
        *result = Some(status);
    }
    callback.completion.changed.notify_all();
    S_OK
}
unsafe extern "system" fn marker(_: *mut c_void, _: u32, _: *const c_void) -> HRESULT {
    S_OK
}

static VTABLE: IMFSinkWriterCallback_Vtbl = IMFSinkWriterCallback_Vtbl {
    base__: IUnknown_Vtbl {
        QueryInterface: query,
        AddRef: add_ref,
        Release: release,
    },
    OnFinalize: finalized,
    OnMarker: marker,
};

pub(super) fn callback() -> (IMFSinkWriterCallback, Arc<Completion>) {
    let completion = Arc::new(Completion::default());
    let object = Box::new(Callback {
        vtable: &VTABLE,
        references: AtomicU32::new(1),
        completion: completion.clone(),
    });
    // SAFETY: repr(C) places the static COM vtable first. The initial reference
    // is transferred to the interface; QueryInterface/AddRef/Release own it.
    let interface = unsafe { IMFSinkWriterCallback::from_raw(Box::into_raw(object).cast()) };
    (interface, completion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows62::Win32::Foundation::E_FAIL;
    struct WorkerCallback(IMFSinkWriterCallback);
    // SAFETY: only wraps our own agile callback created by callback(), never
    // an arbitrary apartment-bound COM implementation.
    unsafe impl Send for WorkerCallback {}
    impl WorkerCallback {
        fn signal(self) {
            unsafe {
                self.0.OnFinalize(S_OK).unwrap();
            }
        }
    }
    #[test]
    fn callback_balances_references_and_reports_completion_errors() {
        let (interface, completion) = callback();
        let clone = interface.clone();
        let unknown: IUnknown = interface.cast().unwrap();
        assert_eq!(Arc::strong_count(&completion), 2);
        unsafe {
            clone.OnFinalize(E_FAIL).unwrap();
            clone.OnFinalize(S_OK).unwrap();
        }
        assert!(
            format!(
                "{:#}",
                completion
                    .wait(&|| false, Duration::from_secs(1))
                    .unwrap_err()
            )
            .contains("0x80004005")
        );
        drop(interface);
        drop(clone);
        drop(unknown);
        assert_eq!(Arc::strong_count(&completion), 1);
    }
    #[test]
    fn cancellation_timeout_and_worker_callback_are_observed() {
        let (interface, completion) = callback();
        assert!(
            completion
                .wait(&|| true, Duration::from_secs(1))
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(
            completion
                .wait(&|| false, Duration::from_millis(1))
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        let interface = WorkerCallback(interface);
        let callback_thread = std::thread::spawn(move || interface.signal());
        completion.wait(&|| false, Duration::from_secs(1)).unwrap();
        callback_thread.join().unwrap();
    }
}
