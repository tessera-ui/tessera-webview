use std::{cell::RefCell, rc::Rc};

use gdkx11::{X11Display, X11Window};
use gtk::prelude::*;
use tessera_ui::{
    PluginContext, PxSize,
    platform_view::{LinuxPlatformViewFactory, WaylandChildSurface, X11ChildWindow},
};

use crate::{WebViewError, WebViewEvent, WebViewOptions, WebViewSource, plugin::EventSink};

use super::{Command, error};

#[path = "webkit_linux.rs"]
mod webkit_linux;
use webkit_linux::WebKit;

#[cfg(tessera_wpe)]
#[path = "wayland_webkit.rs"]
mod wayland_webkit;

pub(crate) fn pump() {
    let context = gtk::glib::MainContext::default();
    for _ in 0..64 {
        if !context.iteration(false) {
            break;
        }
    }
}

#[derive(Clone)]
pub(crate) struct Factory(Rc<State>);

pub(crate) struct NativeView(Factory);

struct State {
    #[cfg(tessera_wpe)]
    window: std::sync::Arc<tessera_ui::winit::window::Window>,
    source: RefCell<WebViewSource>,
    options: WebViewOptions,
    sink: EventSink,
    browser: RefCell<Option<Browser>>,
}

struct Browser {
    host: Host,
    view: WebKit,
}

enum Host {
    X11(gtk::Window),
    #[cfg(tessera_wpe)]
    Wayland(Box<wayland_webkit::Surface>),
}

impl NativeView {
    pub(crate) fn new(
        _context: &PluginContext,
        source: &WebViewSource,
        options: &WebViewOptions,
        sink: EventSink,
    ) -> Result<Self, WebViewError> {
        Ok(Self(Factory(Rc::new(State {
            #[cfg(tessera_wpe)]
            window: _context.window_handle(),
            source: RefCell::new(source.clone()),
            options: options.clone(),
            sink,
            browser: RefCell::new(None),
        }))))
    }

    pub(crate) fn factory(&self) -> Factory {
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

    pub(crate) fn load(&self, source: &WebViewSource) -> Result<(), WebViewError> {
        if let Some(browser) = self.0.0.browser.borrow().as_ref() {
            browser.load(source)?;
        }
        self.0.0.source.replace(source.clone());
        Ok(())
    }

    pub(crate) fn url(&self) -> Result<String, WebViewError> {
        self.browser(Browser::url)
    }
    pub(crate) fn can_go_back(&self) -> Result<bool, WebViewError> {
        self.browser(Browser::can_go_back)
    }
    pub(crate) fn can_go_forward(&self) -> Result<bool, WebViewError> {
        self.browser(Browser::can_go_forward)
    }
    pub(crate) fn evaluate(&self, script: &str, request: u64) -> Result<(), WebViewError> {
        self.browser(|browser| browser.evaluate(script, request))
    }
    pub(crate) fn command(&self, command: Command) -> Result<(), WebViewError> {
        self.browser(|browser| browser.command(command))
    }
}

impl LinuxPlatformViewFactory for Factory {
    fn create(&self, child: X11ChildWindow) {
        match Browser::new(
            child,
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
    }

    fn place_x11(&self, _: X11ChildWindow, size: Option<PxSize>) {
        if let Some(browser) = self.0.browser.borrow().as_ref() {
            let host = match &browser.host {
                Host::X11(host) => host,
                #[cfg(tessera_wpe)]
                Host::Wayland(_) => return,
            };
            if let Some(size) = size {
                let scale = host.scale_factor().max(1);
                host.resize(
                    (size.width.0 / scale).max(1),
                    (size.height.0 / scale).max(1),
                );
                host.show_all();
            } else {
                host.hide();
            }
        }
    }

    fn create_wayland(&self, _surface: WaylandChildSurface) {
        #[cfg(tessera_wpe)]
        let result = (|| {
            let (view, native) = WebKit::wpe(&self.0.options, self.0.sink.clone())?;
            let host = unsafe {
                wayland_webkit::Surface::new(
                    native,
                    _surface,
                    self.0.window.clone(),
                    self.0.sink.clone(),
                )
            };
            view.load(&self.0.source.borrow())?;
            Ok(Browser {
                host: Host::Wayland(Box::new(host)),
                view,
            })
        })();
        #[cfg(not(tessera_wpe))]
        let result = Err(error(
            "Wayland WebView requires building with WPE Platform 2.54 or newer",
        ));
        match result {
            Ok(browser) => {
                self.0.browser.replace(Some(browser));
                self.0.sink.event(WebViewEvent::Ready);
            }
            Err(error) => self.0.sink.event(WebViewEvent::Error { error }),
        }
    }

    #[cfg(tessera_wpe)]
    fn update_wayland(
        &self,
        surface: WaylandChildSurface,
        placement: tessera_ui::WaylandViewPlacement,
    ) {
        if let Some(Browser {
            host: Host::Wayland(host),
            ..
        }) = self.0.browser.borrow().as_ref()
        {
            host.update(surface, placement);
        }
    }

    #[cfg(tessera_wpe)]
    fn hide_wayland(&self, _: WaylandChildSurface) {
        if let Some(Browser {
            host: Host::Wayland(host),
            ..
        }) = self.0.browser.borrow().as_ref()
        {
            host.hide();
        }
    }

    #[cfg(tessera_wpe)]
    fn wayland_buffer_size(&self, _: WaylandChildSurface) -> Option<PxSize> {
        match &self.0.browser.borrow().as_ref()?.host {
            Host::Wayland(host) => host.size(),
            Host::X11(_) => None,
        }
    }

    #[cfg(tessera_wpe)]
    fn wayland_buffer_released(&self, _: WaylandChildSurface, id: u64) {
        if let Some(Browser {
            host: Host::Wayland(host),
            ..
        }) = self.0.browser.borrow().as_ref()
        {
            host.release(id);
        }
    }

    #[cfg(tessera_wpe)]
    fn wayland_buffer_rendered(&self, _: WaylandChildSurface, buffer_id: u64) {
        if let Some(Browser {
            host: Host::Wayland(host),
            ..
        }) = self.0.browser.borrow().as_ref()
        {
            host.rendered(buffer_id);
        }
    }

    fn release_wayland(&self, _: WaylandChildSurface) {
        self.0.browser.take();
    }

    #[cfg(tessera_wpe)]
    fn accepts_wayland_input(&self) -> bool {
        true
    }

    #[cfg(tessera_wpe)]
    fn input_wayland(
        &self,
        _: WaylandChildSurface,
        event: tessera_ui::LinuxViewInputEvent,
        modifiers: tessera_ui::winit::keyboard::ModifiersState,
    ) -> bool {
        if let Some(Browser {
            host: Host::Wayland(host),
            ..
        }) = self.0.browser.borrow().as_ref()
        {
            return host.input(event, modifiers);
        }
        false
    }

    #[cfg(tessera_wpe)]
    fn wayland_ime_request(&self, _: WaylandChildSurface) -> Option<tessera_ui::ImeRequest> {
        match self.0.browser.borrow().as_ref() {
            Some(Browser {
                host: Host::Wayland(host),
                ..
            }) => host.ime_request(),
            _ => None,
        }
    }

    fn release(&self, _: X11ChildWindow) {
        if let Some(Browser {
            host: Host::X11(host),
            ..
        }) = self.0.browser.take()
        {
            host.close();
            gtk::prelude::WidgetExt::display(&host).sync();
        }
    }
}

impl Browser {
    fn new(
        child: X11ChildWindow,
        source: &WebViewSource,
        options: &WebViewOptions,
        sink: EventSink,
    ) -> Result<Self, WebViewError> {
        gtk::init().map_err(error)?;
        let display = gtk::gdk::Display::default()
            .filter(|display| display.is::<X11Display>())
            .or_else(|| {
                let name = std::env::var("DISPLAY").ok()?;
                gtk::gdk::Display::open(&name)
            })
            .filter(|display| display.is::<X11Display>())
            .ok_or_else(|| error("GTK could not open the X11 display"))?;
        let x11_display = display.clone().downcast::<X11Display>().map_err(error)?;
        let host = gtk::Window::new(gtk::WindowType::Popup);
        host.set_screen(&display.default_screen());
        host.set_decorated(false);
        host.set_accept_focus(true);
        host.set_focus_on_map(false);
        host.set_default_size(1, 1);
        host.realize();
        let window = host
            .window()
            .ok_or_else(|| error("GTK did not realize an X11 window"))?;
        let parent = X11Window::foreign_new_for_display(&x11_display, child.window());
        window.ensure_native();
        window.reparent(parent.as_ref(), 0, 0);
        display.flush();
        let view = WebKit::gtk(options, sink)?;
        host.add(view.widget());
        view.load(source)?;
        let widget = view.widget();
        widget.connect_button_press_event(|widget, event| {
            if let Some(window) = widget.toplevel().and_then(|host| host.window()) {
                window.focus(event.time());
            }
            widget.grab_focus();
            gtk::glib::Propagation::Proceed
        });
        Ok(Self {
            view,
            host: Host::X11(host),
        })
    }

    pub fn load(&self, source: &WebViewSource) -> Result<(), WebViewError> {
        self.view.load(source)
    }
    pub fn url(&self) -> Result<String, WebViewError> {
        Ok(self.view.url())
    }
    pub fn can_go_back(&self) -> Result<bool, WebViewError> {
        Ok(self.view.can_go_back())
    }
    pub fn can_go_forward(&self) -> Result<bool, WebViewError> {
        Ok(self.view.can_go_forward())
    }
    pub fn evaluate(&self, script: &str, request: u64) -> Result<(), WebViewError> {
        self.view.evaluate(script, Some(request))
    }
    pub fn command(&self, command: Command) -> Result<(), WebViewError> {
        match command {
            Command::Script(script) => self.view.evaluate(&script, None),
            Command::Back => {
                self.view.back();
                Ok(())
            }
            Command::Forward => {
                self.view.forward();
                Ok(())
            }
            Command::Reload => {
                self.view.reload();
                Ok(())
            }
            Command::Focus => {
                match &self.host {
                    Host::X11(host) => {
                        if let Some(window) = host.window().filter(|window| window.is_viewable()) {
                            window.focus(0);
                        }
                        self.view.widget().grab_focus();
                    }
                    #[cfg(tessera_wpe)]
                    Host::Wayland(host) => host.request_focus(),
                }
                Ok(())
            }
            Command::Zoom(value) => {
                self.view.zoom(value);
                Ok(())
            }
            Command::ClearData => {
                self.view.clear_data();
                Ok(())
            }
        }
    }
}
