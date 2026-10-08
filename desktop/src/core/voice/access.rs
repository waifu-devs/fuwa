//! Whether the system lets the app use the microphone and camera. Only macOS
//! asks: the first time, it shows its own prompt (with the words from the
//! bundle's Info.plist), and once someone says no, only System Settings
//! changes it. Without asking, macOS hands the app silence and says nothing.

/// What the system says about a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Allowed,
    /// The system's prompt is up; ask again in a moment.
    Asking,
    /// Turned off in the system's settings.
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Microphone,
    Camera,
}

/// Whether the app may use `device`, showing the system's prompt the first time.
pub fn check(device: Device) -> Access {
    imp::check(device)
}

/// Whether there are system settings to send someone to when it's blocked.
pub fn has_settings() -> bool {
    cfg!(target_os = "macos")
}

/// Opens the system's privacy settings for `device`.
pub fn open_settings(device: Device) {
    imp::open_settings(device);
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{
        AVAuthorizationStatus, AVCaptureDevice, AVMediaType, AVMediaTypeAudio, AVMediaTypeVideo,
    };
    use objc2_foundation::{NSBundle, NSString};

    use super::{Access, Device};

    /// Asked once per run: the answer is read back through the status.
    static ASKED: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

    fn kind(device: Device) -> Option<&'static AVMediaType> {
        // SAFETY: AVFoundation's constants, set once it's loaded.
        unsafe {
            match device {
                Device::Microphone => AVMediaTypeAudio,
                Device::Camera => AVMediaTypeVideo,
            }
        }
    }

    /// macOS ends an app that asks without saying why in its Info.plist; run
    /// outside a bundle (cargo run), the terminal answers for it instead.
    fn may_ask(device: Device) -> bool {
        let key = match device {
            Device::Microphone => "NSMicrophoneUsageDescription",
            Device::Camera => "NSCameraUsageDescription",
        };
        NSBundle::mainBundle().objectForInfoDictionaryKey(&NSString::from_str(key)).is_some()
    }

    pub fn check(device: Device) -> Access {
        let Some(kind) = kind(device) else { return Access::Allowed };
        // SAFETY: audio and video are the two types this takes; it's safe from any thread.
        let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(kind) };
        match status {
            AVAuthorizationStatus::Authorized => Access::Allowed,
            AVAuthorizationStatus::NotDetermined if !may_ask(device) => Access::Allowed,
            AVAuthorizationStatus::NotDetermined => {
                if !ASKED[device as usize].swap(true, Ordering::Relaxed) {
                    let answered = RcBlock::new(|_: Bool| {});
                    // SAFETY: as above; the block is copied by AVFoundation.
                    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(kind, &answered) };
                }
                Access::Asking
            }
            _ => Access::Blocked,
        }
    }

    pub fn open_settings(device: Device) {
        let page = match device {
            Device::Microphone => "Privacy_Microphone",
            Device::Camera => "Privacy_Camera",
        };
        let _ = open::that_detached(format!("x-apple.systempreferences:com.apple.preference.security?{page}"));
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Access, Device};

    pub fn check(_: Device) -> Access {
        Access::Allowed
    }

    pub fn open_settings(_: Device) {}
}
