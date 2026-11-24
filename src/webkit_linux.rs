use std::{
    ffi::{CStr, CString, c_char, c_int, c_uint, c_void},
    ptr::{null, null_mut},
    sync::OnceLock,
};

use gtk::{
    gio::ffi as gio,
    glib::{self, EnumValue, Object, Type, ffi, gobject_ffi as gobject, translate::*},
    prelude::*,
};
use libloading::Library;

use crate::{
    IpcPolicy, WebViewError, WebViewEvent, WebViewOptions, WebViewSource, plugin::EventSink,
};

use super::super::error;

type Ptr = *mut c_void;
type Str = *const c_char;
type AsyncCallback = gio::GAsyncReadyCallback;
type RegisterGtk = unsafe extern "C" fn(Ptr, Str) -> ffi::gboolean;
type JsResultGtk = unsafe extern "C" fn(Ptr) -> Ptr;

#[derive(Clone, Copy)]
enum PortApi {
    Gtk {
        register: RegisterGtk,
        js_result: JsResultGtk,
        data_manager: unsafe extern "C" fn(Ptr) -> Ptr,
        set_background: unsafe extern "C" fn(Ptr, *const gtk::gdk::ffi::GdkRGBA),
    },
    #[cfg(tessera_wpe)]
    Wpe {
        register: unsafe extern "C" fn(Ptr, Str, Str) -> ffi::gboolean,
        view: unsafe extern "C" fn(Ptr) -> Ptr,
        session: unsafe extern "C" fn(Ptr) -> Ptr,
        ephemeral: unsafe extern "C" fn() -> Ptr,
        data_manager: unsafe extern "C" fn(Ptr) -> Ptr,
        set_background: unsafe extern "C" fn(Ptr, *const [f64; 4]),
    },
}

impl PortApi {
    fn data_manager(&self, view: Ptr) -> Ptr {
        match self {
            Self::Gtk { data_manager, .. } => unsafe { data_manager(view) },
            #[cfg(tessera_wpe)]
            Self::Wpe {
                data_manager,
                session,
                ..
            } => unsafe { data_manager(session(view)) },
        }
    }

    fn set_background(&self, view: Ptr, channels: [u8; 4]) {
        match self {
            Self::Gtk { set_background, .. } => {
                let [red, green, blue, alpha] = channels.map(|v| f64::from(v) / 255.0);
                let color = gtk::gdk::ffi::GdkRGBA {
                    red,
                    green,
                    blue,
                    alpha,
                };
                unsafe { set_background(view, &color) };
            }
            #[cfg(tessera_wpe)]
            Self::Wpe { set_background, .. } => {
                let color = channels.map(|v| f64::from(v) / 255.0);
                unsafe { set_background(view, &color) };
            }
        }
    }
}

macro_rules! webkit_api {
    ($($name:ident: $signature:ty),+ $(,)?) => {
        struct Api {
            // Registered GObject classes and callbacks may outlive individual views.
            _library: Library,
            port: PortApi,
            $($name: $signature),+
        }

        impl Api {
            fn load(soname: &str, port: impl FnOnce(&Library) -> Result<PortApi, WebViewError>) -> Result<Self, WebViewError> {
                // Each entry names a public C API and declares its exact ABI.
                unsafe {
                    let library = Library::new(soname).map_err(error)?;
                    Ok(Self {
                        $($name: *library.get::<$signature>(concat!(stringify!($name), "\0").as_bytes()).map_err(error)?,)+
                        port: port(&library)?,
                        _library: library,
                    })
                }
            }
        }
    };
}

webkit_api! {
    webkit_web_view_get_type: unsafe extern "C" fn() -> ffi::GType,
    webkit_web_view_get_context: unsafe extern "C" fn(Ptr) -> Ptr,
    webkit_web_view_get_settings: unsafe extern "C" fn(Ptr) -> Ptr,
    webkit_web_view_get_user_content_manager: unsafe extern "C" fn(Ptr) -> Ptr,
    webkit_web_view_load_uri: unsafe extern "C" fn(Ptr, Str),
    webkit_web_view_load_html: unsafe extern "C" fn(Ptr, Str, Str),
    webkit_web_view_can_go_back: unsafe extern "C" fn(Ptr) -> ffi::gboolean,
    webkit_web_view_can_go_forward: unsafe extern "C" fn(Ptr) -> ffi::gboolean,
    webkit_web_view_go_back: unsafe extern "C" fn(Ptr),
    webkit_web_view_go_forward: unsafe extern "C" fn(Ptr),
    webkit_web_view_reload: unsafe extern "C" fn(Ptr),
    webkit_web_view_evaluate_javascript: unsafe extern "C" fn(Ptr, Str, isize, Str, Str, *mut gio::GCancellable, AsyncCallback, Ptr),
    webkit_web_view_evaluate_javascript_finish: unsafe extern "C" fn(Ptr, *mut gio::GAsyncResult, *mut *mut ffi::GError) -> Ptr,
    webkit_website_data_manager_clear: unsafe extern "C" fn(Ptr, c_uint, i64, *mut gio::GCancellable, AsyncCallback, Ptr),
    webkit_user_script_new: unsafe extern "C" fn(Str, c_int, c_int, *const Str, *const Str) -> Ptr,
    webkit_user_script_unref: unsafe extern "C" fn(Ptr),
    webkit_user_content_manager_add_script: unsafe extern "C" fn(Ptr, Ptr),
    webkit_navigation_policy_decision_get_navigation_action: unsafe extern "C" fn(Ptr) -> Ptr,
    webkit_navigation_action_get_request: unsafe extern "C" fn(Ptr) -> Ptr,
    webkit_uri_request_get_uri: unsafe extern "C" fn(Ptr) -> Str,
    webkit_policy_decision_ignore: unsafe extern "C" fn(Ptr),
    webkit_permission_request_deny: unsafe extern "C" fn(Ptr),
    webkit_download_get_request: unsafe extern "C" fn(Ptr) -> Ptr,
    webkit_download_get_web_view: unsafe extern "C" fn(Ptr) -> Ptr,
    webkit_download_cancel: unsafe extern "C" fn(Ptr),
    jsc_value_is_string: unsafe extern "C" fn(Ptr) -> ffi::gboolean,
    jsc_value_to_string: unsafe extern "C" fn(Ptr) -> *mut c_char,
    jsc_value_to_json: unsafe extern "C" fn(Ptr, c_uint) -> *mut c_char,
}

// WebKitGTK and WPE WebKit export overlapping symbols and GObject types. Do not
// link a port into the executable: select it from the actual native factory.
static GTK_API: OnceLock<Result<Api, WebViewError>> = OnceLock::new();
#[cfg(tessera_wpe)]
static WPE_API: OnceLock<Result<Api, WebViewError>> = OnceLock::new();

const IPC_SCRIPT: &str = r#"Object.defineProperty(window, 'ipc', {
  value: Object.freeze({postMessage: function (body) {
    window.webkit.messageHandlers.tessera.postMessage(String(body));
  }})
});"#;

pub(super) struct WebKit {
    api: &'static Api,
    view: Object,
    context: Object,
    download_handler: Option<glib::SignalHandlerId>,
    sink: EventSink,
}

impl WebKit {
    pub(super) fn gtk(options: &WebViewOptions, sink: EventSink) -> Result<Self, WebViewError> {
        ensure_port(true)?;
        let api = GTK_API
            .get_or_init(|| {
                Api::load("libwebkit2gtk-4.1.so.0", |lib| unsafe {
                    Ok(PortApi::Gtk {
                        register: *lib
                            .get(b"webkit_user_content_manager_register_script_message_handler\0")
                            .map_err(error)?,
                        js_result: *lib
                            .get(b"webkit_javascript_result_get_js_value\0")
                            .map_err(error)?,
                        data_manager: *lib
                            .get(b"webkit_web_view_get_website_data_manager\0")
                            .map_err(error)?,
                        set_background: *lib
                            .get(b"webkit_web_view_set_background_color\0")
                            .map_err(error)?,
                    })
                })
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let view_type = unsafe { Type::from_glib((api.webkit_web_view_get_type)()) };
        let view = Object::builder_with_type(view_type)
            .property("is-ephemeral", options.incognito)
            .build();
        Self::new(api, view, options, sink)
    }

    #[cfg(tessera_wpe)]
    pub(super) fn wpe(
        options: &WebViewOptions,
        sink: EventSink,
    ) -> Result<(Self, Ptr), WebViewError> {
        ensure_port(false)?;
        let api = WPE_API
            .get_or_init(|| {
                Api::load("libWPEWebKit-2.0.so.1", |lib| unsafe {
                    let port = PortApi::Wpe {
                        register: *lib
                            .get(b"webkit_user_content_manager_register_script_message_handler\0")
                            .map_err(error)?,
                        view: *lib.get(b"webkit_web_view_get_wpe_view\0").map_err(error)?,
                        session: *lib
                            .get(b"webkit_web_view_get_network_session\0")
                            .map_err(error)?,
                        ephemeral: *lib
                            .get(b"webkit_network_session_new_ephemeral\0")
                            .map_err(error)?,
                        data_manager: *lib
                            .get(b"webkit_network_session_get_website_data_manager\0")
                            .map_err(error)?,
                        set_background: *lib
                            .get(b"webkit_web_view_set_background_color\0")
                            .map_err(error)?,
                    };
                    super::wayland_webkit::load(lib)?;
                    Ok(port)
                })
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let PortApi::Wpe {
            view: wpe_view,
            ephemeral,
            ..
        } = api.port
        else {
            unreachable!()
        };
        let display = super::wayland_webkit::display()?;
        let view_type = unsafe { Type::from_glib((api.webkit_web_view_get_type)()) };
        let mut builder = Object::builder_with_type(view_type).property("display", display);
        if options.incognito {
            let session: Object = unsafe { from_glib_full(ephemeral().cast()) };
            builder = builder.property("network-session", session);
        }
        let view = builder.build();
        let native = unsafe { wpe_view(ptr(&view)) };
        if native.is_null() {
            return Err(error("WPE WebKit did not create a platform view"));
        }
        Ok((Self::new(api, view, options, sink)?, native))
    }

    fn new(
        api: &'static Api,
        view: Object,
        options: &WebViewOptions,
        sink: EventSink,
    ) -> Result<Self, WebViewError> {
        if matches!(options.ipc, IpcPolicy::Origins(_)) {
            return Err(WebViewError::UnsupportedOperation(
                "origin-verified IPC on Linux WebKit; use Disabled or AllowAll for trusted content"
                    .into(),
            ));
        }
        let settings: Object =
            unsafe { from_glib_none((api.webkit_web_view_get_settings)(ptr(&view)).cast()) };
        settings.set_property("enable-javascript", options.javascript);
        settings.set_property("enable-developer-extras", options.devtools);
        if let Some(agent) = &options.user_agent {
            settings.set_property("user-agent", agent);
        }
        api.port.set_background(ptr(&view), options.background);
        let context: Object = unsafe {
            from_glib_none(
                match api.port {
                    PortApi::Gtk { .. } => (api.webkit_web_view_get_context)(ptr(&view)),
                    #[cfg(tessera_wpe)]
                    PortApi::Wpe { session, .. } => session(ptr(&view)),
                }
                .cast(),
            )
        };
        let mut browser = Self {
            api,
            view,
            context,
            download_handler: None,
            sink,
        };
        browser.events(options);
        if options.ipc != IpcPolicy::Disabled {
            browser.enable_messages(options.ipc.clone())?;
        }
        for script in &options.initialization_scripts {
            browser.add_script(script)?;
        }
        Ok(browser)
    }

    pub(super) fn widget(&self) -> &gtk::Widget {
        self.view
            .downcast_ref()
            .expect("WebKitGTK view is a GtkWidget")
    }

    fn manager(&self) -> Ptr {
        unsafe { (self.api.webkit_web_view_get_user_content_manager)(ptr(&self.view)) }
    }

    fn add_script(&self, script: &str) -> Result<(), WebViewError> {
        let script = CString::new(script).map_err(error)?;
        unsafe {
            // All frames, injected at document start.
            let script = (self.api.webkit_user_script_new)(script.as_ptr(), 0, 0, null(), null());
            (self.api.webkit_user_content_manager_add_script)(self.manager(), script);
            (self.api.webkit_user_script_unref)(script);
        }
        Ok(())
    }

    fn enable_messages(&self, policy: IpcPolicy) -> Result<(), WebViewError> {
        let api = self.api;
        let sink = self.sink.clone();
        let weak = self.view.downgrade();
        let manager: Object = unsafe { from_glib_none(self.manager().cast()) };
        manager.connect_local("script-message-received::tessera", false, move |args| {
            let view = weak.upgrade()?;
            let url = uri(&view);
            if !policy.allows(&url) {
                return None;
            }
            let value = unsafe {
                match api.port {
                    PortApi::Gtk { js_result, .. } => {
                        js_result(gobject::g_value_get_boxed(args[1].to_glib_none().0))
                    }
                    #[cfg(tessera_wpe)]
                    PortApi::Wpe { .. } => {
                        gobject::g_value_get_object(args[1].to_glib_none().0).cast()
                    }
                }
            };
            if let Some(body) = js_string(api, value)
                && body.len() <= 1024 * 1024
            {
                sink.event(WebViewEvent::Message { url, body });
            }
            None
        });
        if unsafe {
            match api.port {
                PortApi::Gtk { register, .. } => register(self.manager(), c"tessera".as_ptr()),
                #[cfg(tessera_wpe)]
                PortApi::Wpe { register, .. } => {
                    register(self.manager(), c"tessera".as_ptr(), null())
                }
            }
        } == 0
        {
            return Err(error(
                "could not register the WebKit script message handler",
            ));
        }
        self.add_script(IPC_SCRIPT)
    }

    fn events(&mut self, options: &WebViewOptions) {
        let api = self.api;
        let sink = self.sink.clone();
        self.view.connect_local("load-changed", false, move |args| {
            let view = args[0].get::<Object>().ok()?;
            let (_, stage) = EnumValue::from_value(&args[1])?;
            match stage.nick() {
                "started" => sink.event(WebViewEvent::LoadStarted { url: uri(&view) }),
                "finished" => sink.event(WebViewEvent::LoadFinished { url: uri(&view) }),
                _ => (),
            }
            None
        });
        let sink = self.sink.clone();
        self.view
            .connect_notify_local(Some("title"), move |view, _| {
                if let Some(title) = view.property::<Option<String>>("title") {
                    sink.event(WebViewEvent::TitleChanged { title });
                }
            });
        let policy = options.navigation.clone();
        let sink = self.sink.clone();
        self.view
            .connect_local("decide-policy", false, move |args| {
                let (_, kind) = EnumValue::from_value(&args[2])?;
                let decision = args[1].get::<Object>().ok()?;
                let popup = kind.nick() == "new-window-action";
                if kind.nick() != "navigation-action" && !popup {
                    return Some(false.to_value());
                }
                let url = unsafe {
                    let action = (api.webkit_navigation_policy_decision_get_navigation_action)(
                        ptr(&decision),
                    );
                    request_uri(api, (api.webkit_navigation_action_get_request)(action))
                };
                if popup {
                    sink.event(WebViewEvent::NewWindowRequested { url });
                } else if !policy.allows(&url) {
                    sink.event(WebViewEvent::NavigationBlocked { url });
                } else {
                    return Some(false.to_value());
                }
                unsafe { (api.webkit_policy_decision_ignore)(ptr(&decision)) };
                Some(true.to_value())
            });
        self.view
            .connect_local("permission-request", false, move |args| {
                let request = unsafe { gobject::g_value_peek_pointer(args[1].to_glib_none().0) };
                if !request.is_null() {
                    unsafe { (api.webkit_permission_request_deny)(request) };
                }
                Some(true.to_value())
            });
        let sink = self.sink.clone();
        let weak = self.view.downgrade();
        self.download_handler = Some(self.context.connect_local(
            "download-started",
            false,
            move |args| {
                let view = weak.upgrade()?;
                let download = args[1].get::<Object>().ok()?;
                unsafe {
                    if (api.webkit_download_get_web_view)(ptr(&download)) == ptr(&view) {
                        let url =
                            request_uri(api, (api.webkit_download_get_request)(ptr(&download)));
                        (api.webkit_download_cancel)(ptr(&download));
                        sink.event(WebViewEvent::DownloadRequested { url });
                    }
                }
                None
            },
        ));
    }

    pub(super) fn load(&self, source: &WebViewSource) -> Result<(), WebViewError> {
        let view = ptr(&self.view);
        unsafe {
            match source {
                WebViewSource::Url(url) => (self.api.webkit_web_view_load_uri)(
                    view,
                    CString::new(url.as_str()).map_err(error)?.as_ptr(),
                ),
                WebViewSource::Html(html) => (self.api.webkit_web_view_load_html)(
                    view,
                    CString::new(html.as_str()).map_err(error)?.as_ptr(),
                    null(),
                ),
            }
        }
        Ok(())
    }

    pub(super) fn url(&self) -> String {
        uri(&self.view)
    }

    pub(super) fn can_go_back(&self) -> bool {
        unsafe { (self.api.webkit_web_view_can_go_back)(ptr(&self.view)) != 0 }
    }

    pub(super) fn can_go_forward(&self) -> bool {
        unsafe { (self.api.webkit_web_view_can_go_forward)(ptr(&self.view)) != 0 }
    }

    pub(super) fn evaluate(&self, script: &str, request: Option<u64>) -> Result<(), WebViewError> {
        let script = CString::new(script).map_err(error)?;
        let (callback, data): (AsyncCallback, Ptr) = if let Some(request) = request {
            let pending = Box::new(Evaluation {
                api: self.api,
                sink: self.sink.clone(),
                request,
            });
            (Some(evaluated), Box::into_raw(pending).cast())
        } else {
            (None, null_mut())
        };
        unsafe {
            (self.api.webkit_web_view_evaluate_javascript)(
                ptr(&self.view),
                script.as_ptr(),
                -1,
                null(),
                null(),
                null_mut(),
                callback,
                data,
            );
        }
        Ok(())
    }

    pub(super) fn back(&self) {
        unsafe { (self.api.webkit_web_view_go_back)(ptr(&self.view)) }
    }

    pub(super) fn forward(&self) {
        unsafe { (self.api.webkit_web_view_go_forward)(ptr(&self.view)) }
    }

    pub(super) fn reload(&self) {
        unsafe { (self.api.webkit_web_view_reload)(ptr(&self.view)) }
    }

    pub(super) fn zoom(&self, value: f64) {
        self.view.set_property("zoom-level", value);
    }

    pub(super) fn clear_data(&self) {
        const ALL_WEBSITE_DATA: c_uint = (1 << 14) - 1;
        unsafe {
            let manager = self.api.port.data_manager(ptr(&self.view));
            (self.api.webkit_website_data_manager_clear)(
                manager,
                ALL_WEBSITE_DATA,
                0,
                null_mut(),
                None,
                null_mut(),
            );
        }
    }
}

fn ensure_port(gtk: bool) -> Result<(), WebViewError> {
    if let Some(registered) = Type::from_name("WebKitWebView")
        && registered.is_a(gtk::Widget::static_type()) != gtk
    {
        return Err(error(
            "A different WebKit port is already active in this process",
        ));
    }
    Ok(())
}

impl Drop for WebKit {
    fn drop(&mut self) {
        if let Some(handler) = self.download_handler.take() {
            self.context.disconnect(handler);
        }
    }
}

struct Evaluation {
    api: &'static Api,
    sink: EventSink,
    request: u64,
}

unsafe extern "C" fn evaluated(
    source: *mut gobject::GObject,
    result: *mut gio::GAsyncResult,
    data: Ptr,
) {
    unsafe {
        let Evaluation { api, sink, request } = *Box::from_raw(data.cast());
        let value =
            (api.webkit_web_view_evaluate_javascript_finish)(source.cast(), result, null_mut());
        let json = if value.is_null() {
            None
        } else {
            let json = take_string((api.jsc_value_to_json)(value, 0));
            gobject::g_object_unref(value.cast());
            json
        };
        sink.script(request, json.unwrap_or_else(|| "null".into()));
    }
}

fn js_string(api: &Api, value: Ptr) -> Option<String> {
    unsafe {
        if value.is_null() || (api.jsc_value_is_string)(value) == 0 {
            return None;
        }
        take_string((api.jsc_value_to_string)(value))
    }
}

unsafe fn take_string(text: *mut c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    unsafe {
        let value = CStr::from_ptr(text).to_string_lossy().into_owned();
        ffi::g_free(text.cast());
        Some(value)
    }
}

unsafe fn request_uri(api: &Api, request: Ptr) -> String {
    unsafe {
        let uri = (api.webkit_uri_request_get_uri)(request);
        if uri.is_null() {
            String::new()
        } else {
            CStr::from_ptr(uri).to_string_lossy().into_owned()
        }
    }
}

fn ptr(object: &Object) -> Ptr {
    object.as_ptr().cast()
}

fn uri(view: &Object) -> String {
    view.property::<Option<String>>("uri").unwrap_or_default()
}
