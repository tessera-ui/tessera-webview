use tessera_ui::{FloatRect, PluginContext};
use wry::{
    NewWindowResponse, PageLoadEvent, PermissionResponse, Rect, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

use crate::{
    IpcPolicy, WebViewError, WebViewEvent, WebViewOptions, WebViewSource, plugin::EventSink,
};

use super::{Command, error};

pub(crate) fn initialize(_context: &PluginContext) -> Result<(), WebViewError> {
    #[cfg(target_os = "linux")]
    gtk::init().map_err(error)?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn pump(
    _context: &PluginContext,
    _sender: &async_channel::Sender<crate::plugin::Envelope>,
) {
    // Bound each turn so a busy GTK source cannot starve Tessera's frame work.
    for _ in 0..64 {
        if !gtk::events_pending() {
            break;
        }
        gtk::main_iteration_do(false);
    }
}

pub(crate) struct NativeView {
    view: WebView,
    sink: EventSink,
    #[cfg(target_os = "windows")]
    inline_navigation: std::rc::Rc<std::cell::RefCell<Option<String>>>,
}

impl NativeView {
    pub fn new(
        context: &PluginContext,
        source: &WebViewSource,
        options: &WebViewOptions,
        bounds: FloatRect,
        sink: EventSink,
    ) -> Result<Self, WebViewError> {
        #[cfg(target_os = "linux")]
        if matches!(options.ipc, IpcPolicy::Origins(_)) {
            // Wry/WebKitGTK attributes iframe messages to the top-level URL.
            // Do not advertise an origin boundary that the backend cannot prove.
            return Err(WebViewError::UnsupportedOperation(
                "origin-verified IPC on WebKitGTK; use Disabled or AllowAll for trusted content"
                    .into(),
            ));
        }
        let load = sink.clone();
        let title = sink.clone();
        let navigate = sink.clone();
        let popup = sink.clone();
        let download = sink.clone();
        let policy = options.navigation.clone();
        #[cfg(target_os = "windows")]
        let inline_navigation = std::rc::Rc::new(std::cell::RefCell::new(match source {
            WebViewSource::Html(html) => Some(inline_url(html)),
            _ => None,
        }));
        #[cfg(target_os = "windows")]
        let inline_permission = inline_navigation.clone();
        let [r, g, b, a] = options.background;
        let mut builder = WebViewBuilder::new()
            .with_visible(false)
            .with_focused(false)
            .with_devtools(options.devtools)
            .with_incognito(options.incognito)
            .with_background_color((r, g, b, a))
            .with_bounds(to_bounds(bounds, context.window().scale_factor()))
            .with_navigation_handler(move |url| {
                // WebView2 reports NavigateToString as a data URL. Permit only
                // the exact document supplied by Rust, never arbitrary data URLs.
                #[cfg(target_os = "windows")]
                if inline_permission.borrow().as_deref() == Some(url.as_str()) {
                    inline_permission.borrow_mut().take();
                    return true;
                }
                let allowed = policy.allows(&url);
                if !allowed {
                    navigate.event(WebViewEvent::NavigationBlocked { url });
                }
                allowed
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_new_window_req_handler(move |url, _| {
                popup.event(WebViewEvent::NewWindowRequested { url });
                NewWindowResponse::Deny
            })
            .with_download_started_handler(move |url, _| {
                download.event(WebViewEvent::DownloadRequested { url });
                false
            })
            .with_document_title_changed_handler(move |title_value| {
                title.event(WebViewEvent::TitleChanged { title: title_value })
            })
            .with_on_page_load_handler(move |event, url| {
                load.event(match event {
                    PageLoadEvent::Started => WebViewEvent::LoadStarted { url },
                    PageLoadEvent::Finished => WebViewEvent::LoadFinished { url },
                })
            });
        if !options.javascript {
            builder = builder.with_javascript_disabled();
        }
        if let Some(agent) = &options.user_agent {
            builder = builder.with_user_agent(agent);
        }
        for script in &options.initialization_scripts {
            builder = builder.with_initialization_script(script);
        }
        if options.ipc != IpcPolicy::Disabled {
            let ipc = options.ipc.clone();
            let messages = sink.clone();
            builder = builder.with_ipc_handler(move |request| {
                let url = request.uri().to_string();
                if request.body().len() <= 1024 * 1024 && ipc.allows(&url) {
                    messages.event(WebViewEvent::Message {
                        url,
                        body: request.into_body(),
                    });
                }
            });
        }
        builder = match source {
            WebViewSource::Url(url) => builder.with_url(url),
            WebViewSource::Html(html) => builder.with_html(html),
        };
        #[cfg(not(target_os = "ios"))]
        let view = builder.build_as_child(context.window()).map_err(error)?;
        #[cfg(target_os = "ios")]
        let view = builder.build(context.window()).map_err(error)?;
        let native = Self {
            view,
            sink,
            #[cfg(target_os = "windows")]
            inline_navigation,
        };
        native.set_bounds(bounds, context.window().scale_factor())?;
        native.sink.event(WebViewEvent::Ready);
        Ok(native)
    }

    pub fn set_bounds(&self, bounds: FloatRect, scale: f64) -> Result<(), WebViewError> {
        #[cfg(not(target_os = "ios"))]
        self.view
            .set_bounds(to_bounds(bounds, scale))
            .map_err(error)?;
        #[cfg(target_os = "ios")]
        {
            use objc2_core_foundation::{CGPoint, CGRect, CGSize};
            use objc2_ui_kit::UIViewAutoresizing;
            use wry::WebViewExtIOS;
            let view = self.view.webview();
            // Wry does not implement iOS child bounds; disable autoresizing and
            // use UIKit points on the main thread that owns this native view.
            view.setAutoresizingMask(UIViewAutoresizing::None);
            view.setFrame(CGRect::new(
                CGPoint::new(f64::from(bounds.x) / scale, f64::from(bounds.y) / scale),
                CGSize::new(
                    f64::from(bounds.width) / scale,
                    f64::from(bounds.height) / scale,
                ),
            ));
        }
        Ok(())
    }

    pub fn set_visible(&self, visible: bool) -> Result<(), WebViewError> {
        self.view.set_visible(visible).map_err(error)
    }
    pub fn bounds(&self, scale: f64) -> Result<FloatRect, WebViewError> {
        #[cfg(not(target_os = "ios"))]
        {
            let bounds = self.view.bounds().map_err(error)?;
            let position = bounds.position.to_physical::<f64>(scale);
            let size = bounds.size.to_physical::<f64>(scale);
            Ok(FloatRect {
                x: position.x as f32,
                y: position.y as f32,
                width: size.width as f32,
                height: size.height as f32,
            })
        }
        #[cfg(target_os = "ios")]
        {
            use wry::WebViewExtIOS;
            let frame = self.view.webview().frame();
            Ok(FloatRect {
                x: (frame.origin.x * scale) as f32,
                y: (frame.origin.y * scale) as f32,
                width: (frame.size.width * scale) as f32,
                height: (frame.size.height * scale) as f32,
            })
        }
    }
    pub fn load(&self, source: &WebViewSource) -> Result<(), WebViewError> {
        #[cfg(target_os = "windows")]
        {
            *self.inline_navigation.borrow_mut() = match source {
                WebViewSource::Html(html) => Some(inline_url(html)),
                _ => None,
            };
        }
        match source {
            WebViewSource::Url(url) => self.view.load_url(url),
            WebViewSource::Html(html) => self.view.load_html(html),
        }
        .map_err(error)
    }
    pub fn url(&self) -> Result<String, WebViewError> {
        self.view.url().map_err(error)
    }
    pub fn can_go_back(&self) -> Result<bool, WebViewError> {
        self.view.can_go_back().map_err(error)
    }
    pub fn can_go_forward(&self) -> Result<bool, WebViewError> {
        self.view.can_go_forward().map_err(error)
    }
    pub fn evaluate(&self, script: &str, request: u64) -> Result<(), WebViewError> {
        let sink = self.sink.clone();
        self.view
            .evaluate_script_with_callback(script, move |value| sink.script(request, value))
            .map_err(error)
    }
    pub fn command(&self, command: Command) -> Result<(), WebViewError> {
        match command {
            Command::Script(script) => self.view.evaluate_script(&script),
            Command::Back => self.view.go_back(),
            Command::Forward => self.view.go_forward(),
            Command::Reload => self.view.reload(),
            Command::Focus => {
                #[cfg(target_os = "ios")]
                {
                    use wry::WebViewExtIOS;
                    if !self.view.webview().becomeFirstResponder() {
                        return Err(WebViewError::Native("WKWebView refused focus".into()));
                    }
                    return Ok(());
                }
                #[cfg(not(target_os = "ios"))]
                self.view.focus()
            }
            Command::Zoom(value) => self.view.zoom(value),
            Command::ClearData => self.view.clear_all_browsing_data(),
        }
        .map_err(error)
    }
}

#[cfg(target_os = "windows")]
fn inline_url(html: &str) -> String {
    use base64::Engine;
    format!(
        "data:text/html;charset=utf-8;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(html)
    )
}

fn to_bounds(bounds: FloatRect, scale: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(f64::from(bounds.x) / scale, f64::from(bounds.y) / scale)
            .into(),
        size: LogicalSize::new(
            f64::from(bounds.width) / scale,
            f64::from(bounds.height) / scale,
        )
        .into(),
    }
}
