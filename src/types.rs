use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

/// A URL or an inline HTML document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WebViewSource {
    /// An absolute HTTP or HTTPS URL.
    Url(String),
    /// An inline document. Never interpolate untrusted text without escaping it.
    Html(String),
}

impl WebViewSource {
    /// Creates a URL source.
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url(url.into())
    }
    /// Creates an inline HTML source.
    pub fn html(html: impl Into<String>) -> Self {
        Self::Html(html.into())
    }

    pub(crate) fn validate(&self, policy: &NavigationPolicy) -> Result<(), WebViewError> {
        if let Self::Url(url) = self
            && (!is_web_url(url) || !policy.allows(url))
        {
            return Err(WebViewError::NavigationDenied(url.clone()));
        }
        Ok(())
    }
}

/// Allowed top-level destinations. Subresource requests are not a network sandbox.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "origins", rename_all = "snake_case")]
pub enum NavigationPolicy {
    /// HTTP(S) destinations and `about:blank`; external schemes are blocked.
    #[default]
    Web,
    /// Only these exact HTTP(S) origins, plus `about:blank` for inline HTML.
    Origins(Vec<String>),
}

impl NavigationPolicy {
    pub(crate) fn allows(&self, value: &str) -> bool {
        value == "about:blank"
            || match self {
                Self::Web => is_web_url(value),
                Self::Origins(origins) => origin_matches(origins, value),
            }
    }
}

/// Controls which pages may send `window.ipc.postMessage(string)` messages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "origins", rename_all = "snake_case")]
pub enum IpcPolicy {
    /// No JavaScript-to-Rust bridge is installed.
    #[default]
    Disabled,
    /// Only exact HTTP(S) origins, including their effective port.
    Origins(Vec<String>),
    /// All origins, including opaque inline HTML. Use only with trusted content.
    AllowAll,
}

impl IpcPolicy {
    #[cfg(any(
        test,
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "ios"
    ))]
    pub(crate) fn allows(&self, url: &str) -> bool {
        match self {
            Self::Disabled => false,
            Self::AllowAll => true,
            Self::Origins(origins) => origin_matches(origins, url),
        }
    }
}

/// Engine settings. Changing these recreates the view and resets its history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebViewOptions {
    /// Top-level navigation restrictions.
    pub navigation: NavigationPolicy,
    /// JavaScript-to-Rust access; disabled by default.
    pub ipc: IpcPolicy,
    /// Scripts executed at document creation, before page scripts where supported.
    /// Android falls back to page start on older System WebView versions, where
    /// execution before page scripts is not guaranteed.
    pub initialization_scripts: Vec<String>,
    /// Optional browser user agent override.
    pub user_agent: Option<String>,
    /// Whether page JavaScript is enabled. Defaults to `true`.
    pub javascript: bool,
    /// Allows engine developer tools. Defaults to `false`.
    pub devtools: bool,
    /// Uses an ephemeral data store. Not supported on Android.
    pub incognito: bool,
    /// Background RGBA bytes. Defaults to opaque white.
    pub background: [u8; 4],
}

impl Default for WebViewOptions {
    fn default() -> Self {
        Self {
            navigation: NavigationPolicy::Web,
            ipc: IpcPolicy::Disabled,
            initialization_scripts: Vec::new(),
            user_agent: None,
            javascript: true,
            devtools: false,
            incognito: false,
            background: [255; 4],
        }
    }
}

impl WebViewOptions {
    pub(crate) fn validate(&self) -> Result<(), WebViewError> {
        let navigation = match &self.navigation {
            NavigationPolicy::Origins(values) => values.as_slice(),
            _ => &[],
        };
        let ipc = match &self.ipc {
            IpcPolicy::Origins(values) => values.as_slice(),
            _ => &[],
        };
        for origin in navigation.iter().chain(ipc) {
            let parsed = Url::parse(origin).map_err(|_| {
                WebViewError::InvalidConfiguration(format!("invalid origin: {origin}"))
            })?;
            if !is_web_url(origin)
                || parsed.path() != "/"
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return Err(WebViewError::InvalidConfiguration(format!(
                    "expected an HTTP(S) origin without credentials or a path: {origin}"
                )));
            }
        }
        Ok(())
    }
}

fn is_web_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn origin_matches(origins: &[String], value: &str) -> bool {
    let Ok(value) = Url::parse(value) else {
        return false;
    };
    if !is_web_url(value.as_str()) {
        return false;
    }
    origins
        .iter()
        .filter_map(|origin| Url::parse(origin).ok())
        .any(|allowed| allowed.origin() == value.origin())
}

/// Browser events delivered outside the native engine's call stack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WebViewEvent {
    /// The native view is available; emitted again after recreation or resume.
    Ready,
    /// Navigation began.
    LoadStarted { url: String },
    /// The engine reported page load completion, not necessarily HTTP success.
    LoadFinished { url: String },
    /// The document's title changed.
    TitleChanged { title: String },
    /// A permitted page posted a string, limited to one MiB.
    Message { url: String, body: String },
    /// The navigation policy rejected a destination.
    NavigationBlocked { url: String },
    /// A popup was requested. It was blocked; the app decides how to handle it.
    NewWindowRequested { url: String },
    /// A download was requested. Automatic downloads are disabled.
    DownloadRequested { url: String },
    /// A configuration, lifecycle, or engine operation failed.
    Error { error: WebViewError },
}

/// Failures from the component and native browser engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebViewError {
    /// No native backend is available on this target.
    UnsupportedPlatform,
    /// This operation is unavailable on the selected engine.
    UnsupportedOperation(String),
    /// The plugin was not registered, or no owning UI session is active.
    PluginNotRegistered,
    /// The renderer is suspended.
    HostNotResumed,
    /// The component has not mounted or was removed.
    NotMounted,
    /// Layout or asynchronous platform creation has not completed.
    NotReady,
    /// Two components attempted to own the same controller.
    ControllerInUse,
    /// The view was removed, suspended, or recreated before completion.
    Cancelled,
    /// JavaScript did not complete within its deadline.
    EvaluationTimedOut,
    /// Invalid options or numeric input.
    InvalidConfiguration(String),
    /// A requested URL was rejected.
    NavigationDenied(String),
    /// A native platform operation failed.
    Native(String),
}

impl fmt::Display for WebViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => f.write_str("no WebView backend for this target"),
            Self::UnsupportedOperation(value) => write!(f, "unsupported operation: {value}"),
            Self::PluginNotRegistered => {
                f.write_str("register WebViewPlugin and use the owning UI session")
            }
            Self::HostNotResumed => f.write_str("the host renderer is suspended"),
            Self::NotMounted => f.write_str("the WebView component is not mounted"),
            Self::NotReady => f.write_str("the native WebView is not ready"),
            Self::ControllerInUse => f.write_str("a controller can own only one mounted WebView"),
            Self::Cancelled => f.write_str("the WebView operation was cancelled"),
            Self::EvaluationTimedOut => f.write_str("JavaScript evaluation timed out"),
            Self::InvalidConfiguration(value) => {
                write!(f, "invalid WebView configuration: {value}")
            }
            Self::NavigationDenied(value) => write!(f, "navigation denied: {value}"),
            Self::Native(value) => f.write_str(value),
        }
    }
}

impl std::error::Error for WebViewError {}
