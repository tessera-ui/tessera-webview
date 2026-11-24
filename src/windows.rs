use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

use futures_channel::oneshot;
use tessera_ui::{
    PluginContext, PxPosition, Task,
    platform_view::{
        NativeVisual, WindowsPlatformViewFactory, WindowsViewInputEvent, WindowsViewPlacement,
    },
    spawn_local,
    winit::{
        event::{MouseButton, MouseScrollDelta, TouchPhase},
        keyboard::ModifiersState,
    },
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::{
    Win32::{
        Foundation::{E_ABORT, E_NOTIMPL, E_POINTER, HWND, POINT, RECT},
        Graphics::DirectComposition::IDCompositionDevice,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        UI::{
            Input::Pointer::{
                POINTER_FLAG_CANCELED, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT,
                POINTER_FLAG_INRANGE, POINTER_FLAG_UP, POINTER_FLAG_UPDATE,
            },
            WindowsAndMessaging::{
                HCURSOR, IDC_ARROW, LoadCursorW, PT_TOUCH, SetCursor, TOUCH_MASK_CONTACTAREA,
            },
        },
    },
    core::{BOOL, HSTRING, IUnknown, Interface, PCWSTR, PWSTR, Result},
};

use crate::{
    IpcPolicy, WebViewError, WebViewEvent, WebViewOptions, WebViewSource, plugin::EventSink,
};

use super::{Command, error};

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

thread_local! {
    static APARTMENT: Result<Apartment> = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }.map(|_| Apartment);
}

#[derive(Clone)]
pub(crate) struct Factory(Rc<State>);

pub(crate) struct NativeView(Factory);

struct State {
    source: RefCell<WebViewSource>,
    options: WebViewOptions,
    sink: EventSink,
    view: RefCell<Option<Rc<View>>>,
    placement: Cell<Option<WindowsViewPlacement>>,
    startup: RefCell<Option<Task>>,
}

struct View {
    pointer_environment: ICoreWebView2Environment3,
    controller: ICoreWebView2Controller,
    composition: ICoreWebView2CompositionController,
    web: ICoreWebView2,
    inline: Rc<RefCell<Option<String>>>,
    buttons: RefCell<HashSet<MouseButton>>,
    touches: RefCell<HashMap<u32, POINT>>,
    last_point: Cell<POINT>,
    hovered: Rc<Cell<bool>>,
}

impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            let _ = self.composition.SetRootVisualTarget(None::<&IUnknown>);
            let _ = self.controller.Close();
        }
    }
}

impl NativeView {
    pub(crate) fn new(
        _: &PluginContext,
        source: &WebViewSource,
        options: &WebViewOptions,
        sink: EventSink,
    ) -> std::result::Result<Self, WebViewError> {
        Ok(Self(Factory(Rc::new(State {
            source: RefCell::new(source.clone()),
            options: options.clone(),
            sink,
            view: RefCell::new(None),
            placement: Cell::new(None),
            startup: RefCell::new(None),
        }))))
    }

    pub(crate) fn factory(&self) -> Factory {
        self.0.clone()
    }

    fn view(&self) -> std::result::Result<Rc<View>, WebViewError> {
        self.0.0.view.borrow().clone().ok_or(WebViewError::NotReady)
    }

    pub(crate) fn load(&self, source: &WebViewSource) -> std::result::Result<(), WebViewError> {
        if let Ok(view) = self.view() {
            view.load(source).map_err(error)?;
        }
        self.0.0.source.replace(source.clone());
        Ok(())
    }

    pub(crate) fn url(&self) -> std::result::Result<String, WebViewError> {
        self.view()?.url().map_err(error)
    }

    pub(crate) fn can_go_back(&self) -> std::result::Result<bool, WebViewError> {
        let mut value = BOOL::default();
        unsafe { self.view()?.web.CanGoBack(&mut value) }.map_err(error)?;
        Ok(value.as_bool())
    }

    pub(crate) fn can_go_forward(&self) -> std::result::Result<bool, WebViewError> {
        let mut value = BOOL::default();
        unsafe { self.view()?.web.CanGoForward(&mut value) }.map_err(error)?;
        Ok(value.as_bool())
    }

    pub(crate) fn evaluate(
        &self,
        script: &str,
        request: u64,
    ) -> std::result::Result<(), WebViewError> {
        let sink = self.0.0.sink.clone();
        self.view()?
            .script(script, move |result, value| {
                sink.script(request, if result.is_ok() { value } else { "null".into() });
                Ok(())
            })
            .map_err(error)
    }

    pub(crate) fn command(&self, command: Command) -> std::result::Result<(), WebViewError> {
        let view = self.view()?;
        unsafe {
            match command {
                Command::Script(script) => view.script(&script, |_, _| Ok(())),
                Command::Back => view.web.GoBack(),
                Command::Forward => view.web.GoForward(),
                Command::Reload => view.web.Reload(),
                Command::Focus => view.focus(),
                Command::Zoom(value) => view.controller.SetZoomFactor(value),
                Command::ClearData => view
                    .web
                    .cast::<ICoreWebView2_13>()
                    .and_then(|web| web.Profile())
                    .and_then(|profile| profile.cast::<ICoreWebView2Profile2>())
                    .and_then(|profile| {
                        profile.ClearBrowsingDataAll(&ClearBrowsingDataCompletedHandler::create(
                            Box::new(|_| Ok(())),
                        ))
                    }),
            }
        }
        .map_err(error)
    }
}

impl WindowsPlatformViewFactory for Factory {
    fn create(&self, _: &IDCompositionDevice) -> Result<NativeVisual> {
        Err(E_NOTIMPL.into())
    }

    fn create_in_window(&self, device: &IDCompositionDevice, parent: HWND) -> Result<NativeVisual> {
        let visual = unsafe { device.CreateVisual()? };
        let state = Rc::downgrade(&self.0);
        let sink = self.0.sink.clone();
        let options = self.0.options.clone();
        let device = device.clone();
        let target = visual.clone();
        let task = spawn_local(async move {
            let result = async {
                APARTMENT.with(|apartment| apartment.as_ref().map(|_| ()).map_err(Clone::clone))?;
                let environment = environment().await?;
                let (controller, composition) =
                    controller(&environment, parent, options.incognito).await?;
                let web = match unsafe { controller.CoreWebView2() } {
                    Ok(web) => web,
                    Err(err) => {
                        let _ = unsafe { controller.Close() };
                        return Err(err);
                    }
                };
                let view = Rc::new(View {
                    pointer_environment: environment.cast::<ICoreWebView2Environment3>()?,
                    controller,
                    composition,
                    web,
                    inline: Default::default(),
                    buttons: Default::default(),
                    touches: Default::default(),
                    last_point: Default::default(),
                    hovered: Default::default(),
                });
                view.configure(&options, &sink).await?;
                let Some(state) = state.upgrade() else {
                    return Ok(());
                };
                unsafe {
                    view.composition
                        .SetRootVisualTarget(&target.cast::<IUnknown>()?)?;
                    device.Commit()?;
                }
                view.place(state.placement.get())?;
                view.load(&state.source.borrow())?;
                state.view.replace(Some(view));
                sink.event(WebViewEvent::Ready);
                Ok(())
            }
            .await;
            if let Err(err) = result {
                sink.event(WebViewEvent::Error { error: error(err) });
            }
        });
        self.0.startup.replace(Some(task));
        Ok(visual)
    }

    fn place(&self, _: &NativeVisual, placement: Option<WindowsViewPlacement>) {
        let old = self.0.placement.replace(placement);
        if old != placement
            && let Some(view) = self.0.view.borrow().clone()
            && let Err(err) = view.place(placement)
        {
            self.0.sink.event(WebViewEvent::Error { error: error(err) });
        }
    }

    fn accepts_pointer_input(&self) -> bool {
        true
    }

    fn pointer_input(
        &self,
        _: &NativeVisual,
        event: WindowsViewInputEvent,
        modifiers: ModifiersState,
    ) {
        if let Some(view) = self.0.view.borrow().clone()
            && let Err(err) = view.pointer(event, modifiers)
        {
            self.0.sink.event(WebViewEvent::Error { error: error(err) });
        }
    }

    fn release(&self, _: NativeVisual) {
        self.0.startup.replace(None);
        self.0.view.replace(None);
        self.0.placement.set(None);
    }
}

async fn environment() -> Result<ICoreWebView2Environment> {
    let (tx, rx) = oneshot::channel();
    let data = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("tessera-webview")
        .join("WebView2");
    let data = HSTRING::from(data.as_os_str());
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &data,
            None,
            &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
                move |result, environment| {
                    let _ =
                        tx.send(result.and_then(|_| environment.ok_or_else(|| E_POINTER.into())));
                    Ok(())
                },
            )),
        )?;
    }
    rx.await.map_err(|_| E_ABORT)?
}

async fn controller(
    environment: &ICoreWebView2Environment,
    parent: HWND,
    incognito: bool,
) -> Result<(ICoreWebView2Controller, ICoreWebView2CompositionController)> {
    let (tx, rx) = oneshot::channel();
    let callback = CreateCoreWebView2CompositionControllerCompletedHandler::create(Box::new(
        move |result, composition| {
            let result = result.and_then(|_| {
                let composition = composition.ok_or(E_POINTER)?;
                Ok((composition.cast::<ICoreWebView2Controller>()?, composition))
            });
            if let Err(Ok((controller, _))) = tx.send(result) {
                let _ = unsafe { controller.Close() };
            }
            Ok(())
        },
    ));
    unsafe {
        if incognito {
            let environment = environment.cast::<ICoreWebView2Environment10>()?;
            let options = environment.CreateCoreWebView2ControllerOptions()?;
            options.SetIsInPrivateModeEnabled(true)?;
            environment
                .CreateCoreWebView2CompositionControllerWithOptions(parent, &options, &callback)?;
        } else {
            environment
                .cast::<ICoreWebView2Environment3>()?
                .CreateCoreWebView2CompositionController(parent, &callback)?;
        }
    }
    rx.await.map_err(|_| E_ABORT)?
}

fn string(get: impl FnOnce(&mut PWSTR) -> Result<()>) -> Result<String> {
    let mut value = PWSTR::null();
    let result = get(&mut value);
    let value = take_pwstr(value);
    result.map(|_| value)
}

impl View {
    async fn configure(&self, options: &WebViewOptions, sink: &EventSink) -> Result<()> {
        let mut token = 0;
        unsafe {
            self.controller.SetIsVisible(false)?;
            let settings = self.web.Settings()?;
            settings.SetAreDevToolsEnabled(options.devtools)?;
            settings.SetIsScriptEnabled(options.javascript)?;
            settings.SetIsWebMessageEnabled(options.ipc != IpcPolicy::Disabled)?;
            if let Some(agent) = &options.user_agent {
                settings
                    .cast::<ICoreWebView2Settings2>()?
                    .SetUserAgent(&HSTRING::from(agent))?;
            }
            let [r, g, b, alpha] = options.background;
            self.controller
                .cast::<ICoreWebView2Controller2>()?
                .SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                    R: r,
                    G: g,
                    B: b,
                    A: if alpha == 0 { 0 } else { 255 },
                })?;

            let navigation = options.navigation.clone();
            let inline = self.inline.clone();
            let events = sink.clone();
            self.web.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let args = args.ok_or(E_POINTER)?;
                    let url = string(|value| args.Uri(value))?;
                    let inline_allowed =
                        inline.borrow_mut().take().as_deref() == Some(url.as_str());
                    if inline_allowed || navigation.allows(&url) {
                        events.event(WebViewEvent::LoadStarted { url });
                    } else {
                        args.SetCancel(true)?;
                        events.event(WebViewEvent::NavigationBlocked { url });
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let events = sink.clone();
            self.web.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                    let mut success = BOOL::default();
                    args.ok_or(E_POINTER)?.IsSuccess(&mut success)?;
                    if success.as_bool() {
                        let sender = sender.ok_or(E_POINTER)?;
                        events.event(WebViewEvent::LoadFinished {
                            url: string(|value| sender.Source(value))?,
                        });
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let events = sink.clone();
            self.web.add_DocumentTitleChanged(
                &DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
                    let sender = sender.ok_or(E_POINTER)?;
                    events.event(WebViewEvent::TitleChanged {
                        title: string(|value| sender.DocumentTitle(value))?,
                    });
                    Ok(())
                })),
                &mut token,
            )?;
            self.web.add_PermissionRequested(
                &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                    args.ok_or(E_POINTER)?
                        .SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)
                })),
                &mut token,
            )?;
            let events = sink.clone();
            self.web.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                    let args = args.ok_or(E_POINTER)?;
                    args.SetHandled(true)?;
                    events.event(WebViewEvent::NewWindowRequested {
                        url: string(|value| args.Uri(value))?,
                    });
                    Ok(())
                })),
                &mut token,
            )?;
            let events = sink.clone();
            self.web.cast::<ICoreWebView2_4>()?.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    let args = args.ok_or(E_POINTER)?;
                    args.SetCancel(true)?;
                    args.SetHandled(true)?;
                    let download = args.DownloadOperation()?;
                    events.event(WebViewEvent::DownloadRequested {
                        url: string(|value| download.Uri(value))?,
                    });
                    Ok(())
                })),
                &mut token,
            )?;
            let hovered = self.hovered.clone();
            self.composition.add_CursorChanged(
                &CursorChangedEventHandler::create(Box::new(move |sender, _| {
                    if hovered.get()
                        && let Some(sender) = sender
                    {
                        set_cursor(&sender)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;

            if options.ipc != IpcPolicy::Disabled {
                let policy = options.ipc.clone();
                let events = sink.clone();
                self.web.add_WebMessageReceived(
                    &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                        let args = args.ok_or(E_POINTER)?;
                        let url = string(|value| args.Source(value))?;
                        if policy.allows(&url)
                            && let Ok(body) = string(|value| args.TryGetWebMessageAsString(value))
                            && body.len() <= 1024 * 1024
                        {
                            events.event(WebViewEvent::Message { url, body });
                        }
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
        }
        if options.ipc != IpcPolicy::Disabled {
            self.install_script("Object.defineProperty(window, 'ipc', { value: Object.freeze({ postMessage: function(s) { window.chrome.webview.postMessage(s) } }) });").await?;
        }
        for script in &options.initialization_scripts {
            self.install_script(script).await?;
        }
        Ok(())
    }

    async fn install_script(&self, script: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        unsafe {
            self.web.AddScriptToExecuteOnDocumentCreated(
                &HSTRING::from(script),
                &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                    move |result, _| {
                        let _ = tx.send(result);
                        Ok(())
                    },
                )),
            )?;
        }
        rx.await.map_err(|_| E_ABORT)?
    }

    fn script(
        &self,
        script: &str,
        callback: impl FnOnce(Result<()>, String) -> Result<()> + 'static,
    ) -> Result<()> {
        unsafe {
            self.web.ExecuteScript(
                &HSTRING::from(script),
                &ExecuteScriptCompletedHandler::create(Box::new(callback)),
            )
        }
    }

    fn url(&self) -> Result<String> {
        string(|value| unsafe { self.web.Source(value) })
    }

    fn load(&self, source: &WebViewSource) -> Result<()> {
        use base64::Engine as _;
        let expected = match source {
            WebViewSource::Html(html) => Some(format!(
                "data:text/html;charset=utf-8;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(html)
            )),
            _ => None,
        };
        self.inline.replace(expected);
        let result = unsafe {
            match source {
                WebViewSource::Url(url) => self.web.Navigate(&HSTRING::from(url)),
                WebViewSource::Html(html) => self.web.NavigateToString(&HSTRING::from(html)),
            }
        };
        if result.is_err() {
            self.inline.replace(None);
        }
        result
    }

    fn place(&self, placement: Option<WindowsViewPlacement>) -> Result<()> {
        unsafe {
            if let Some(placement) = placement {
                let coordinates = placement.coordinates;
                let bounds = coordinates.window_bounds();
                let left = bounds.x.round() as i32;
                let top = bounds.y.round() as i32;
                self.controller.SetBounds(RECT {
                    left,
                    top,
                    right: left.saturating_add(coordinates.local_size.width.0.max(1)),
                    bottom: top.saturating_add(coordinates.local_size.height.0.max(1)),
                })?;
            }
            self.controller.SetIsVisible(placement.is_some())
        }
    }

    fn focus(&self) -> Result<()> {
        unsafe {
            self.controller
                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
        }
    }

    fn send_touch(&self, id: u32, position: POINT, phase: TouchPhase) -> Result<()> {
        let (event, flags) = match phase {
            TouchPhase::Started => (
                COREWEBVIEW2_POINTER_EVENT_KIND_DOWN,
                POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
            ),
            TouchPhase::Moved => (
                COREWEBVIEW2_POINTER_EVENT_KIND_UPDATE,
                POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
            ),
            TouchPhase::Ended => (COREWEBVIEW2_POINTER_EVENT_KIND_UP, POINTER_FLAG_UP),
            TouchPhase::Cancelled => (
                COREWEBVIEW2_POINTER_EVENT_KIND_UP,
                POINTER_FLAG_UP | POINTER_FLAG_CANCELED,
            ),
        };
        unsafe {
            let pointer = self.pointer_environment.CreateCoreWebView2PointerInfo()?;
            pointer.SetPointerKind(PT_TOUCH.0 as u32)?;
            pointer.SetPointerId(id)?;
            pointer.SetPointerFlags(flags.0)?;
            pointer.SetPixelLocation(position)?;
            pointer.SetPixelLocationRaw(position)?;
            pointer.SetTouchMask(TOUCH_MASK_CONTACTAREA)?;
            let contact = RECT {
                left: position.x,
                top: position.y,
                right: position.x.saturating_add(1),
                bottom: position.y.saturating_add(1),
            };
            pointer.SetTouchContact(contact)?;
            pointer.SetTouchContactRaw(contact)?;
            self.composition.SendPointerInput(event, &pointer)
        }
    }

    fn pointer(&self, event: WindowsViewInputEvent, modifiers: ModifiersState) -> Result<()> {
        let point = |p: PxPosition| POINT {
            x: p.x.round() as i32,
            y: p.y.round() as i32,
        };
        match event {
            WindowsViewInputEvent::Move(position) => {
                self.hovered.set(true);
                set_cursor(&self.composition)?;
                self.mouse(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
                    0,
                    point(position),
                    modifiers,
                )
            }
            WindowsViewInputEvent::Button {
                position,
                button,
                state,
            } => self.mouse_button(button, state.is_pressed(), point(position), modifiers, true),
            WindowsViewInputEvent::Wheel { position, delta } => {
                let (mut x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 120., y * 120.),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                x = -x;
                for (kind, delta) in [
                    (COREWEBVIEW2_MOUSE_EVENT_KIND_HORIZONTAL_WHEEL, x),
                    (COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL, y),
                ] {
                    if delta.round() != 0. {
                        self.mouse(
                            kind,
                            delta.round() as i32 as u32,
                            point(position),
                            modifiers,
                        )?;
                    }
                }
                Ok(())
            }
            WindowsViewInputEvent::Touch(touch) => {
                let position = point(PxPosition::from_f64_arr2([
                    touch.location.x,
                    touch.location.y,
                ]));
                let id = touch.id as u32;
                match touch.phase {
                    TouchPhase::Started => {
                        self.focus()?;
                        self.touches.borrow_mut().insert(id, position);
                        self.send_touch(id, position, touch.phase)?;
                    }
                    TouchPhase::Moved => {
                        if self.touches.borrow().contains_key(&id) {
                            self.touches.borrow_mut().insert(id, position);
                            self.send_touch(id, position, touch.phase)?;
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        if self.touches.borrow_mut().remove(&id).is_some() {
                            self.send_touch(id, position, touch.phase)?;
                        }
                    }
                }
                Ok(())
            }
            WindowsViewInputEvent::Cancel => {
                let buttons: Vec<_> = self.buttons.borrow().iter().copied().collect();
                let touches = self.touches.borrow_mut().drain().collect::<Vec<_>>();
                let point = self.last_point.get();
                self.leave()?;
                for button in buttons {
                    self.mouse_button(button, false, POINT { x: -1, y: -1 }, modifiers, false)?;
                }
                for (id, position) in touches {
                    self.send_touch(id, position, TouchPhase::Cancelled)?;
                }
                self.last_point.set(point);
                Ok(())
            }
            WindowsViewInputEvent::Leave => self.leave(),
            _ => Ok(()),
        }
    }

    fn leave(&self) -> Result<()> {
        if self.hovered.replace(false) {
            unsafe {
                SetCursor(Some(LoadCursorW(None, IDC_ARROW)?));
                self.composition.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_LEAVE,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                    0,
                    POINT::default(),
                )?;
            }
        }
        Ok(())
    }

    fn mouse_button(
        &self,
        button: MouseButton,
        pressed: bool,
        point: POINT,
        modifiers: ModifiersState,
        focus: bool,
    ) -> Result<()> {
        let (down, up, data) = match button {
            MouseButton::Left => (
                COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN,
                COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP,
                0,
            ),
            MouseButton::Right => (
                COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN,
                COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP,
                0,
            ),
            MouseButton::Middle => (
                COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_DOWN,
                COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_UP,
                0,
            ),
            MouseButton::Back => (
                COREWEBVIEW2_MOUSE_EVENT_KIND_X_BUTTON_DOWN,
                COREWEBVIEW2_MOUSE_EVENT_KIND_X_BUTTON_UP,
                1,
            ),
            MouseButton::Forward => (
                COREWEBVIEW2_MOUSE_EVENT_KIND_X_BUTTON_DOWN,
                COREWEBVIEW2_MOUSE_EVENT_KIND_X_BUTTON_UP,
                2,
            ),
            _ => return Ok(()),
        };
        if pressed {
            if focus {
                self.focus()?;
            }
            self.buttons.borrow_mut().insert(button);
        } else if !self.buttons.borrow_mut().remove(&button) {
            return Ok(());
        }
        self.mouse(if pressed { down } else { up }, data, point, modifiers)
    }

    fn mouse(
        &self,
        kind: COREWEBVIEW2_MOUSE_EVENT_KIND,
        data: u32,
        point: POINT,
        modifiers: ModifiersState,
    ) -> Result<()> {
        self.last_point.set(point);
        let mut keys = 0;
        for (held, flag) in [(modifiers.shift_key(), 4), (modifiers.control_key(), 8)] {
            if held {
                keys |= flag;
            }
        }
        for button in self.buttons.borrow().iter() {
            keys |= match button {
                MouseButton::Left => 1,
                MouseButton::Right => 2,
                MouseButton::Middle => 16,
                MouseButton::Back => 32,
                MouseButton::Forward => 64,
                _ => 0,
            };
        }
        unsafe {
            self.composition.SendMouseInput(
                kind,
                COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS(keys),
                data,
                point,
            )
        }
    }
}

fn set_cursor(composition: &ICoreWebView2CompositionController) -> Result<()> {
    let mut cursor = HCURSOR::default();
    unsafe {
        composition.Cursor(&mut cursor)?;
        SetCursor(Some(cursor));
    }
    Ok(())
}
