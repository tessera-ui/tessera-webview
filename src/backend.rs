use crate::WebViewError;

#[cfg(target_os = "android")]
#[path = "android.rs"]
mod implementation;
#[cfg(target_os = "ios")]
#[path = "ios.rs"]
mod implementation;
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "ios",
    target_os = "android"
)))]
#[path = "unsupported.rs"]
mod implementation;
#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "ios"
))]
#[path = "native.rs"]
mod native;

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub(crate) use implementation::*;
#[cfg(target_os = "linux")]
pub(crate) use native::pump;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) use native::{Browser as NativeView, initialize};

pub(crate) enum Command {
    Script(String),
    Back,
    Forward,
    Reload,
    Focus,
    Zoom(f64),
    ClearData,
}

fn error(error: impl std::fmt::Display) -> WebViewError {
    WebViewError::Native(error.to_string())
}
