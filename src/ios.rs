use std::{cell::RefCell, ptr::NonNull, rc::Rc};

use objc2::{MainThreadMarker, rc::Retained};
use objc2_ui_kit::UIView;
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, UiKitWindowHandle, WindowHandle,
};
use tessera_ui::{PluginContext, ios::platform_views::IosPlatformViewFactory};

use crate::{WebViewError, WebViewEvent, WebViewOptions, WebViewSource, plugin::EventSink};

use super::{Command, native::Browser};

struct Host<'a>(&'a UIView);

impl HasWindowHandle for Host<'_> {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = UiKitWindowHandle::new(NonNull::from(self.0).cast());
        // SAFETY: The borrowed UIView stays alive and is used on the UIKit main thread.
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::UiKit(handle)) })
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

impl IosPlatformViewFactory for Factory {
    fn create(&self, main_thread: MainThreadMarker) -> Retained<UIView> {
        let container = UIView::new(main_thread);
        // Wry installs its WKWebView as a flexible-size child of this UIView.
        // Tessera owns the container's attachment, frame, order, and visibility.
        match Browser::build(
            &Host(&container),
            &self.0.source.borrow(),
            &self.0.options,
            self.0.sink.clone(),
        ) {
            Ok(browser) => {
                self.0.browser.replace(Some(browser));
                self.0.sink.event(WebViewEvent::Ready);
            }
            Err(error) => self.0.sink.event(WebViewEvent::Error { error }),
        }
        container
    }

    fn release(&self, _view: Retained<UIView>, _main_thread: MainThreadMarker) {
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
        self.browser(|view| view.evaluate(script, request))
    }

    pub fn command(&self, command: Command) -> Result<(), WebViewError> {
        self.browser(|view| view.command(command))
    }
}
