#![allow(non_snake_case)]

use std::{cell::RefCell, rc::Rc};

use block2::{Block, RcBlock};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class,
    extern_class, extern_methods, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, ProtocolObject},
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSResponder as Responder, NSView as NativeViewHost,
};
use objc2_foundation::{
    NSDate, NSError, NSJSONSerialization, NSJSONWritingOptions, NSObjectProtocol, NSString, NSURL,
    NSURLRequest,
};
#[cfg(target_os = "ios")]
use objc2_ui_kit::{UIColor, UIResponder as Responder, UIView as NativeViewHost};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationResponse, WKNavigationResponsePolicy, WKScriptMessage, WKScriptMessageHandler,
    WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebViewConfiguration,
    WKWebsiteDataStore,
};
use tessera_ui::PluginContext;
#[cfg(target_os = "ios")]
use tessera_ui::ios::platform_views::IosPlatformViewFactory as HostFactory;
#[cfg(target_os = "macos")]
use tessera_ui::platform_view::MacosPlatformViewFactory as HostFactory;

use crate::{
    IpcPolicy, NavigationPolicy, WebViewError, WebViewEvent, WebViewOptions, WebViewSource,
    plugin::EventSink,
};

use super::Command;

extern_class!(
    #[unsafe(super(NativeViewHost, Responder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[derive(Debug)]
    struct WKWebView;
);

impl WKWebView {
    extern_methods!(
        #[unsafe(method(setNavigationDelegate:))]
        pub unsafe fn setNavigationDelegate(
            &self,
            delegate: Option<&ProtocolObject<dyn WKNavigationDelegate>>,
        );

        #[unsafe(method(loadRequest:))]
        pub unsafe fn loadRequest(&self, request: &NSURLRequest) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(loadHTMLString:baseURL:))]
        pub unsafe fn loadHTMLString_baseURL(
            &self,
            html: &NSString,
            base_url: Option<&NSURL>,
        ) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(URL))]
        pub unsafe fn URL(&self) -> Option<Retained<NSURL>>;

        #[unsafe(method(canGoBack))]
        pub unsafe fn canGoBack(&self) -> bool;

        #[unsafe(method(canGoForward))]
        pub unsafe fn canGoForward(&self) -> bool;

        #[unsafe(method(goBack))]
        pub unsafe fn goBack(&self) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(goForward))]
        pub unsafe fn goForward(&self) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(reload))]
        pub unsafe fn reload(&self) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(evaluateJavaScript:completionHandler:))]
        pub unsafe fn evaluateJavaScript_completionHandler(
            &self,
            script: &NSString,
            completion: Option<&Block<dyn Fn(*mut AnyObject, *mut NSError)>>,
        );

        #[unsafe(method(setCustomUserAgent:))]
        pub unsafe fn setCustomUserAgent(&self, user_agent: Option<&NSString>);

        #[unsafe(method(setPageZoom:))]
        pub unsafe fn setPageZoom(&self, zoom: f64);
    );
}

struct DelegateState {
    sink: EventSink,
    navigation: NavigationPolicy,
    ipc: IpcPolicy,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Rc<DelegateState>]
    struct WebViewDelegate;

    unsafe impl NSObjectProtocol for WebViewDelegate {}

    unsafe impl WKNavigationDelegate for WebViewDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_navigation(
            &self,
            _web_view: &WKWebView,
            action: &WKNavigationAction,
            decision: &Block<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe {
                action
                    .request()
                    .URL()
                    .and_then(|url| url.absoluteString())
                    .map(|url| url.to_string())
            };
            let Some(url) = url else {
                decision.call((WKNavigationActionPolicy::Cancel,));
                return;
            };
            if unsafe { action.targetFrame().is_none() } {
                self.ivars()
                    .sink
                    .event(WebViewEvent::NewWindowRequested { url });
                decision.call((WKNavigationActionPolicy::Cancel,));
                return;
            }
            let allowed = self.ivars().navigation.allows(&url);
            if !allowed {
                self.ivars()
                    .sink
                    .event(WebViewEvent::NavigationBlocked { url });
            }
            decision.call((if allowed {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(
            &self,
            _web_view: &WKWebView,
            response: &WKNavigationResponse,
            decision: &Block<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            let can_display: bool = unsafe { msg_send![response, canShowMIMEType] };
            if !can_display {
                let url = unsafe {
                    response
                        .response()
                        .URL()
                        .and_then(|url| url.absoluteString())
                        .map(|url| url.to_string())
                        .unwrap_or_default()
                };
                self.ivars()
                    .sink
                    .event(WebViewEvent::DownloadRequested { url });
            }
            decision.call((if can_display {
                WKNavigationResponsePolicy::Allow
            } else {
                WKNavigationResponsePolicy::Cancel
            },));
        }

        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn did_start(&self, web_view: &WKWebView, _navigation: &WKNavigation) {
            self.ivars()
                .sink
                .event(WebViewEvent::LoadStarted { url: web_url(web_view) });
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, web_view: &WKWebView, _navigation: &WKNavigation) {
            let url = web_url(web_view);
            self.ivars()
                .sink
                .event(WebViewEvent::LoadFinished { url });
            let sink = self.ivars().sink.clone();
            let callback = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
                if value.is_null() {
                    return;
                }
                let title: Retained<NSString> = unsafe { msg_send![&*value, description] };
                sink.event(WebViewEvent::TitleChanged {
                    title: title.to_string(),
                });
            });
            unsafe {
                web_view.evaluateJavaScript_completionHandler(
                    &NSString::from_str("document.title"),
                    Some(&callback),
                );
            }
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(
            &self,
            _web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.ivars().sink.event(WebViewEvent::Error {
                error: WebViewError::Native(error.localizedDescription().to_string()),
            });
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(
            &self,
            _web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.ivars().sink.event(WebViewEvent::Error {
                error: WebViewError::Native(error.localizedDescription().to_string()),
            });
        }
    }

    unsafe impl WKScriptMessageHandler for WebViewDelegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive_message(
            &self,
            _controller: &WKUserContentController,
            message: &WKScriptMessage,
        ) {
            let name = unsafe { message.name().to_string() };
            let body = unsafe { message.body() };
            let is_string: bool = unsafe { msg_send![&*body, isKindOfClass: NSString::class()] };
            if !is_string {
                return;
            }
            let body: Retained<NSString> = unsafe { msg_send![&*body, description] };
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(&body.to_string()) else {
                return;
            };
            if name == "tesseraTitle" {
                if let Some(title) = payload.get("title").and_then(|value| value.as_str()) {
                    self.ivars().sink.event(WebViewEvent::TitleChanged {
                        title: title.to_owned(),
                    });
                }
                return;
            }
            if name != "tesseraIpc" {
                return;
            }
            let Some(text) = payload.get("body").and_then(|value| value.as_str()) else {
                return;
            };
            if text.len() > 1024 * 1024 {
                return;
            }
            let url = unsafe {
                message
                    .frameInfo()
                    .request()
                    .URL()
                    .and_then(|url| url.absoluteString())
                    .map(|url| url.to_string())
                    .unwrap_or_default()
            };
            if self.ivars().ipc.allows(&url) {
                self.ivars().sink.event(WebViewEvent::Message {
                    url,
                    body: text.to_owned(),
                });
            }
        }
    }
);

fn web_url(web_view: &WKWebView) -> String {
    unsafe {
        web_view
            .URL()
            .and_then(|url| url.absoluteString())
            .map(|url| url.to_string())
            .unwrap_or_default()
    }
}

struct Browser {
    web_view: Retained<WKWebView>,
    delegate: Retained<WebViewDelegate>,
    data_store: Retained<WKWebsiteDataStore>,
}

impl Browser {
    fn new(
        container: &NativeViewHost,
        source: &WebViewSource,
        options: &WebViewOptions,
        sink: EventSink,
        main_thread: MainThreadMarker,
    ) -> Result<Self, WebViewError> {
        let user_content = unsafe { WKUserContentController::new(main_thread) };
        let delegate = main_thread
            .alloc::<WebViewDelegate>()
            .set_ivars(Rc::new(DelegateState {
                sink,
                navigation: options.navigation.clone(),
                ipc: options.ipc.clone(),
            }));
        let delegate: Retained<WebViewDelegate> = unsafe { msg_send![super(delegate), init] };
        let handler = ProtocolObject::from_ref(&*delegate);
        unsafe {
            user_content.addScriptMessageHandler_name(handler, &NSString::from_str("tesseraTitle"));
            user_content.addUserScript(&make_user_script(
                main_thread,
                "(()=>{const send=()=>window.webkit.messageHandlers.tesseraTitle.postMessage(JSON.stringify({title:document.title}));send();new MutationObserver(send).observe(document,{subtree:true,childList:true,characterData:true});})()",
                true,
            ));
            if options.ipc != IpcPolicy::Disabled {
                user_content
                    .addScriptMessageHandler_name(handler, &NSString::from_str("tesseraIpc"));
                user_content.addUserScript(&make_user_script(
                    main_thread,
                    "window.ipc={postMessage:body=>window.webkit.messageHandlers.tesseraIpc.postMessage(JSON.stringify({body:String(body)}))};",
                    false,
                ));
            }
            for script in &options.initialization_scripts {
                user_content.addUserScript(&make_user_script(main_thread, script, false));
            }
        }

        let configuration = unsafe { WKWebViewConfiguration::new(main_thread) };
        unsafe { configuration.setUserContentController(&user_content) };
        let data_store = unsafe {
            if options.incognito {
                WKWebsiteDataStore::nonPersistentDataStore(main_thread)
            } else {
                WKWebsiteDataStore::defaultDataStore(main_thread)
            }
        };
        unsafe { configuration.setWebsiteDataStore(&data_store) };
        if !options.javascript {
            let preferences = unsafe { configuration.defaultWebpagePreferences() };
            unsafe { preferences.setAllowsContentJavaScript(false) };
        }

        let allocated = main_thread.alloc::<WKWebView>();
        let web_view: Retained<WKWebView> = unsafe {
            msg_send![allocated, initWithFrame: container.bounds(), configuration: &*configuration]
        };
        unsafe {
            web_view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            #[cfg(target_os = "ios")]
            web_view.setAutoresizingMask(
                objc2_ui_kit::UIViewAutoresizing::FlexibleWidth
                    | objc2_ui_kit::UIViewAutoresizing::FlexibleHeight,
            );
            #[cfg(target_os = "macos")]
            web_view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            if let Some(agent) = &options.user_agent {
                web_view.setCustomUserAgent(Some(&NSString::from_str(agent)));
            }
            web_view.setPageZoom(1.0);
            let [r, g, b, a] = options.background.map(|channel| f64::from(channel) / 255.0);
            #[cfg(target_os = "ios")]
            {
                let color = UIColor::colorWithRed_green_blue_alpha(r, g, b, a);
                web_view.setOpaque(a == 1.0);
                web_view.setBackgroundColor(Some(&color));
            }
            #[cfg(target_os = "macos")]
            {
                let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
                container.setWantsLayer(true);
                if let Some(layer) = container.layer() {
                    layer.setBackgroundColor(Some(&color.CGColor()));
                }
                let draws_background = objc2_foundation::NSNumber::new_bool(false);
                let _: () = msg_send![&*web_view, setValue: &*draws_background, forKey: &*NSString::from_str("drawsBackground")];
            }
            container.addSubview(&web_view);
            if web_view.respondsToSelector(objc2::sel!(setInspectable:)) {
                let _: () = msg_send![&*web_view, setInspectable: options.devtools];
            }
        }

        let browser = Self {
            web_view,
            delegate,
            data_store,
        };
        browser.load(source)?;
        Ok(browser)
    }

    fn load(&self, source: &WebViewSource) -> Result<(), WebViewError> {
        unsafe {
            match source {
                WebViewSource::Url(value) => {
                    let url = NSURL::URLWithString(&NSString::from_str(value))
                        .ok_or_else(|| WebViewError::NavigationDenied(value.clone()))?;
                    let request = NSURLRequest::requestWithURL(&url);
                    self.web_view.loadRequest(&request);
                }
                WebViewSource::Html(html) => {
                    self.web_view
                        .loadHTMLString_baseURL(&NSString::from_str(html), None);
                }
            }
        }
        Ok(())
    }

    fn url(&self) -> Result<String, WebViewError> {
        Ok(web_url(&self.web_view))
    }

    fn can_go_back(&self) -> Result<bool, WebViewError> {
        Ok(unsafe { self.web_view.canGoBack() })
    }

    fn can_go_forward(&self) -> Result<bool, WebViewError> {
        Ok(unsafe { self.web_view.canGoForward() })
    }

    fn evaluate(&self, script: &str, request: u64) -> Result<(), WebViewError> {
        let sink = self.delegate.ivars().sink.clone();
        let callback = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
            let result = if value.is_null() {
                "null".to_owned()
            } else {
                unsafe {
                    NSJSONSerialization::dataWithJSONObject_options_error(
                        &*value,
                        NSJSONWritingOptions::FragmentsAllowed,
                    )
                    .ok()
                    .and_then(|data| {
                        NSString::initWithData_encoding(
                            NSString::alloc(),
                            &data,
                            objc2_foundation::NSUTF8StringEncoding,
                        )
                    })
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".into())
                }
            };
            sink.script(request, result);
        });
        unsafe {
            self.web_view
                .evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&callback));
        }
        Ok(())
    }

    fn command(&self, command: Command) -> Result<(), WebViewError> {
        unsafe {
            match command {
                Command::Script(script) => {
                    self.web_view
                        .evaluateJavaScript_completionHandler(&NSString::from_str(&script), None);
                }
                Command::Back => {
                    self.web_view.goBack();
                }
                Command::Forward => {
                    self.web_view.goForward();
                }
                Command::Reload => {
                    self.web_view.reload();
                }
                Command::Focus => {
                    #[cfg(target_os = "ios")]
                    let focused = self.web_view.becomeFirstResponder();
                    #[cfg(target_os = "macos")]
                    let focused = self
                        .web_view
                        .window()
                        .is_some_and(|window| window.makeFirstResponder(Some(&self.web_view)));
                    if !focused {
                        return Err(WebViewError::Native("WKWebView refused focus".into()));
                    }
                }
                Command::Zoom(value) => self.web_view.setPageZoom(value),
                Command::ClearData => {
                    let types =
                        WKWebsiteDataStore::allWebsiteDataTypes(MainThreadMarker::new().unwrap());
                    let since = NSDate::distantPast();
                    let completion = RcBlock::new(|| {});
                    self.data_store
                        .removeDataOfTypes_modifiedSince_completionHandler(
                            &types,
                            &since,
                            &completion,
                        );
                }
            }
        }
        Ok(())
    }
}

unsafe fn make_user_script(
    main_thread: MainThreadMarker,
    source: &str,
    main_frame_only: bool,
) -> Retained<WKUserScript> {
    unsafe {
        WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            WKUserScript::alloc(main_thread),
            &NSString::from_str(source),
            WKUserScriptInjectionTime::AtDocumentStart,
            main_frame_only,
        )
    }
}

#[derive(Clone)]
pub(crate) struct Factory(Rc<FactoryState>);

struct FactoryState {
    source: RefCell<WebViewSource>,
    options: WebViewOptions,
    sink: EventSink,
    browser: RefCell<Option<Browser>>,
}

impl HostFactory for Factory {
    fn create(&self, main_thread: MainThreadMarker) -> Retained<NativeViewHost> {
        let container = NativeViewHost::new(main_thread);
        match Browser::new(
            &container,
            &self.0.source.borrow(),
            &self.0.options,
            self.0.sink.clone(),
            main_thread,
        ) {
            Ok(browser) => {
                self.0.browser.replace(Some(browser));
                self.0.sink.event(WebViewEvent::Ready);
            }
            Err(error) => self.0.sink.event(WebViewEvent::Error { error }),
        }
        container
    }

    fn update(&self, view: &NativeViewHost, _main_thread: MainThreadMarker) {
        if let Some(browser) = self.0.browser.borrow().as_ref() {
            browser.web_view.setFrame(view.bounds());
        }
    }

    fn release(&self, _view: Retained<NativeViewHost>, _main_thread: MainThreadMarker) {
        self.0.browser.take();
    }
}

pub(crate) struct NativeView(Factory);

impl NativeView {
    pub fn new(
        _context: &PluginContext,
        source: &WebViewSource,
        options: &WebViewOptions,
        sink: EventSink,
    ) -> Result<Self, WebViewError> {
        Ok(Self(Factory(Rc::new(FactoryState {
            source: RefCell::new(source.clone()),
            options: options.clone(),
            sink,
            browser: RefCell::default(),
        }))))
    }

    pub fn factory(&self) -> Factory {
        self.0.clone()
    }

    fn browser<R>(
        &self,
        f: impl FnOnce(&Browser) -> Result<R, WebViewError>,
    ) -> Result<R, WebViewError> {
        f(self
            .0
            .0
            .browser
            .borrow()
            .as_ref()
            .ok_or(WebViewError::NotReady)?)
    }

    pub fn load(&self, source: &WebViewSource) -> Result<(), WebViewError> {
        if let Some(browser) = self.0.0.browser.borrow().as_ref() {
            browser.load(source)?;
        }
        self.0.0.source.replace(source.clone());
        Ok(())
    }

    pub fn url(&self) -> Result<String, WebViewError> {
        self.browser(Browser::url)
    }

    pub fn can_go_back(&self) -> Result<bool, WebViewError> {
        self.browser(Browser::can_go_back)
    }

    pub fn can_go_forward(&self) -> Result<bool, WebViewError> {
        self.browser(Browser::can_go_forward)
    }

    pub fn evaluate(&self, script: &str, request: u64) -> Result<(), WebViewError> {
        self.browser(|browser| browser.evaluate(script, request))
    }

    pub fn command(&self, command: Command) -> Result<(), WebViewError> {
        self.browser(|browser| browser.command(command))
    }
}
