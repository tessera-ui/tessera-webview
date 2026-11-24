//! Native WebView components for Tessera.
//!
//! Register [`WebViewPackage`] with `EntryPoint::package`, then place
//! [`webview()`] in a bounded layout. See the README for platform-specific
//! composition behavior, prerequisites, and security limitations.

// tessera-macros 1.0 emits mem::replace(..., Some(...)) in generated setters.
#![allow(clippy::mem_replace_option_with_some)]

use std::sync::atomic::{AtomicU64, Ordering};

use tessera_foundation::modifier::ModifierExt as _;
use tessera_ui::{
    CallbackWith, Modifier, Stack, layout::layout, modifier::FocusModifierExt as _, remember,
    remember_with_key, with_plugin,
};
#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "windows",
    target_os = "macos",
    target_os = "linux"
))]
use tessera_ui::{key, platform_view::platform_view};

mod backend;
mod plugin;
mod types;

pub use plugin::{WebViewPackage, WebViewPlugin};
pub use types::{
    IpcPolicy, NavigationPolicy, WebViewError, WebViewEvent, WebViewOptions, WebViewSource,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A stable identity for one mounted WebView. Create it with `remember`.
///
/// Use on its owning UI session. Commands require a mounted, resumed view;
/// [`WebViewEvent::Ready`] indicates that native creation has completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WebViewController(u64);

impl Default for WebViewController {
    fn default() -> Self {
        Self::new()
    }
}

impl WebViewController {
    /// Allocates an independent WebView identity.
    pub fn new() -> Self {
        Self(next_id())
    }

    /// Loads a web URL without changing the component's declarative source.
    pub fn load_url(self, url: impl Into<String>) -> Result<(), WebViewError> {
        let source = WebViewSource::url(url);
        access(|plugin| plugin.load(self.0, source))
    }

    /// Replaces the current document with HTML.
    pub fn load_html(self, html: impl Into<String>) -> Result<(), WebViewError> {
        let source = WebViewSource::html(html);
        access(|plugin| plugin.load(self.0, source))
    }

    /// Schedules JavaScript execution; use [`Self::evaluate`] for its result.
    pub fn evaluate_script(self, script: impl Into<String>) -> Result<(), WebViewError> {
        self.command(backend::Command::Script(script.into()))
    }

    /// Evaluates JavaScript and returns its JSON-encoded result.
    ///
    /// Completion is cancelled on unmount, suspension, or recreation. Engine
    /// exceptions may be reported as `null`; wrap scripts to report errors when
    /// needed. A missing engine response times out after ten seconds.
    pub async fn evaluate(self, script: impl Into<String>) -> Result<String, WebViewError> {
        let response = access(|plugin| plugin.evaluate(self.0, script.into()))?;
        tokio::time::timeout(std::time::Duration::from_secs(10), response)
            .await
            .map_err(|_| WebViewError::EvaluationTimedOut)?
            .map_err(|_| WebViewError::Cancelled)
    }

    /// Navigates backward in history, if possible.
    pub fn go_back(self) -> Result<(), WebViewError> {
        self.command(backend::Command::Back)
    }

    /// Navigates forward in history, if possible.
    pub fn go_forward(self) -> Result<(), WebViewError> {
        self.command(backend::Command::Forward)
    }

    /// Reloads the current document.
    pub fn reload(self) -> Result<(), WebViewError> {
        self.command(backend::Command::Reload)
    }

    /// Gives the native browser keyboard focus.
    pub fn focus(self) -> Result<(), WebViewError> {
        self.command(backend::Command::Focus)
    }

    /// Changes page zoom. The factor must be finite and positive.
    pub fn set_zoom(self, factor: f64) -> Result<(), WebViewError> {
        if !factor.is_finite() || factor <= 0.0 {
            return Err(WebViewError::InvalidConfiguration(
                "zoom must be positive and finite".into(),
            ));
        }
        self.command(backend::Command::Zoom(factor))
    }

    /// Clears the engine's browsing data. Persistent stores may be shared.
    pub fn clear_browsing_data(self) -> Result<(), WebViewError> {
        self.command(backend::Command::ClearData)
    }

    /// Returns the current document URL.
    pub fn url(self) -> Result<String, WebViewError> {
        access(|plugin| plugin.native(self.0)?.url())
    }

    /// Returns whether a backward history entry exists.
    pub fn can_go_back(self) -> Result<bool, WebViewError> {
        access(|plugin| plugin.native(self.0)?.can_go_back())
    }

    /// Returns whether a forward history entry exists.
    pub fn can_go_forward(self) -> Result<bool, WebViewError> {
        access(|plugin| plugin.native(self.0)?.can_go_forward())
    }

    /// Returns the browser's window bounds in physical pixels, before clipping.
    pub fn bounds(self) -> Result<tessera_ui::FloatRect, WebViewError> {
        access(|plugin| plugin.bounds(self.0))
    }

    /// Returns whether the native view currently exists.
    pub fn is_ready(self) -> bool {
        access(|plugin| plugin.native(self.0).map(|_| ())).is_ok()
    }

    fn command(self, command: backend::Command) -> Result<(), WebViewError> {
        access(|plugin| plugin.native(self.0)?.command(command))
    }
}

fn access<R>(
    operation: impl FnOnce(&mut WebViewPlugin) -> Result<R, WebViewError>,
) -> Result<R, WebViewError> {
    with_plugin(operation).unwrap_or(Err(WebViewError::PluginNotRegistered))
}

struct Lease {
    controller: WebViewController,
    owner: u64,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ =
            with_plugin::<WebViewPlugin, _>(|plugin| plugin.detach(self.controller.0, self.owner));
    }
}

/// # webview
///
/// Embeds browser content for documentation, authentication, and hybrid screens.
///
/// ## Usage
///
/// Register [`WebViewPackage`] and place the view in a bounded layout.
///
/// ## Parameters
///
/// - `source` — declarative URL or HTML; changing it navigates the view
/// - `controller` — optional external identity; one mounted component per controller
/// - `options` — security and engine configuration; changes recreate the native view
/// - `on_event` — UI-session callback for load, title, IPC, and error events
/// - `modifier` — layout modifiers; the default fills available space
///
/// ## Examples
///
/// ```
/// use tessera_ui::{EntryPoint, remember, tessera, with_plugin};
/// use tessera_webview::{WebViewController, WebViewPackage, WebViewPlugin, WebViewSource, webview};
///
/// #[tessera]
/// fn app() {
///     let controller = remember(WebViewController::new).get();
///     webview(WebViewSource::html("<h1>Help</h1>"))
///         .controller(controller);
/// }
/// let entry = EntryPoint::new(app).package(WebViewPackage);
/// let _session = entry.runtime_session().enter();
/// assert!(with_plugin::<WebViewPlugin, _>(|_| ()).is_some());
/// assert!(!WebViewController::new().is_ready());
/// ```
#[tessera_ui::tessera]
pub fn webview(
    source: WebViewSource,
    controller: Option<WebViewController>,
    options: Option<WebViewOptions>,
    on_event: Option<CallbackWith<WebViewEvent>>,
    modifier: Option<Modifier>,
) {
    let internal = remember(WebViewController::new).get();
    let controller = controller.unwrap_or(internal);
    let lease = remember_with_key(controller, move || Lease {
        controller,
        owner: next_id(),
    });
    let owner = lease.with(|lease| lease.owner);
    access(|plugin| {
        plugin.configure(
            controller.0,
            owner,
            source,
            options.unwrap_or_default(),
            on_event,
        )
    })
    .expect("webview requires WebViewPackage and a unique mounted controller");
    let modifier = modifier
        .unwrap_or_else(|| Modifier::new().fill_max_size())
        .on_layout_coordinates(move |coordinates: tessera_ui::LayoutCoordinates| {
            let _ =
                access(|plugin| plugin.position(controller.0, owner, coordinates.window_bounds()));
        });
    #[cfg(any(
        target_os = "android",
        target_os = "ios",
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ))]
    if let Ok(Some((generation, factory))) = access(|plugin| plugin.factory(controller.0, owner)) {
        key(generation, || {
            platform_view(factory, modifier);
        });
        return;
    }
    layout(Stack).modifier(modifier);
}
