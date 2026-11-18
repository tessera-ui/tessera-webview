use crate::WebViewError;

#[cfg(target_os = "android")]
#[path = "android.rs"]
mod implementation;
#[cfg(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "linux",
    target_os = "ios"
))]
#[path = "native.rs"]
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

pub(crate) use implementation::*;

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
