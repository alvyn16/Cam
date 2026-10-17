//! Platform and format utility helpers.

static START_INSTANT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn host_now() -> f64 {
    START_INSTANT.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

pub fn secs(t: f64) -> f64 {
    t
}

/// Wrapper asserting a value may cross threads.
pub struct SendBox<T>(pub T);
unsafe impl<T> Send for SendBox<T> {}
unsafe impl<T> Sync for SendBox<T> {}

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

pub const PIX_420V: u32 = fourcc(b"420v");
pub const PIX_BGRA: u32 = fourcc(b"BGRA");

/// Local date-time like "2026-10-11 at 11.30.12" (for file names).
pub fn timestamp_name() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} at {:02}.{:02}.{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}
