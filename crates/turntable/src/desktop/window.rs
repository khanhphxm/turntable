//! Keep the GPUI window and controller alive while its native surface is hidden.
//! Recreating either would lose playback/selection state and media-key ownership.

#[cfg_attr(target_os = "linux", path = "window/linux.rs")]
#[cfg_attr(target_os = "macos", path = "window/macos.rs")]
#[cfg_attr(target_os = "windows", path = "window/windows.rs")]
mod platform;

pub(super) use platform::Visibility;
