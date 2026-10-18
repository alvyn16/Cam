//! Windows desktop capture is ungated; microphone and webcam privacy are checked separately.
use std::time::Duration;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
}
impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::NotDetermined => "not determined",
            Self::Restricted => "restricted",
        })
    }
}
pub fn disclaim_and_reexec() {}
pub fn screen_status() -> Status {
    Status::Granted
}
fn privacy(capability: &str) -> Status {
    let mut status = Status::NotDetermined;
    for root in ["HKLM", "HKCU"] {
        for suffix in ["", r"\NonPackaged"] {
            let key = format!(
                r"{root}\SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{capability}{suffix}"
            );
            let mut cmd = std::process::Command::new("reg.exe");
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
            if let Ok(out) = cmd.args(["query", &key, "/v", "Value"]).output() {
                let value = String::from_utf8_lossy(&out.stdout);
                if value.contains("Deny") {
                    return Status::Denied;
                }
                if value.contains("Allow") {
                    status = Status::Granted;
                }
            }
        }
    }
    status
}
pub fn mic_status() -> Status {
    privacy("microphone")
}
pub fn camera_status() -> Status {
    privacy("webcam")
}
pub fn accessibility_status() -> Status {
    Status::Granted
}
pub fn request_all(_screen: bool, mic: bool, camera: bool, _timeout: Duration) -> bool {
    if mic && mic_status() == Status::Denied {
        crate::app::os::open(std::path::Path::new("ms-settings:privacy-microphone"));
    }
    if camera && camera_status() == Status::Denied {
        crate::app::os::open(std::path::Path::new("ms-settings:privacy-webcam"));
    }
    (!mic || mic_status() != Status::Denied) && (!camera || camera_status() != Status::Denied)
}
