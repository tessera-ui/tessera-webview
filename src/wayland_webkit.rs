use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::{CStr, c_char, c_void},
    ptr::{NonNull, null, null_mut},
    sync::Arc,
};

use gtk::glib::{Object, translate::from_glib_full};
use tessera_ui::{
    ImeRequest, LinuxViewInputEvent, Px, PxPosition, PxSize, WaylandChildSurface,
    WaylandViewPlacement,
    winit::{
        event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase},
        keyboard::{Key, ModifiersState, NamedKey},
        platform::scancode::PhysicalKeyExtScancode,
        window::Window,
    },
};

use crate::{WebViewError, WebViewEvent, plugin::EventSink};

use super::error;

type Ptr = *mut c_void;
type Lookup = unsafe extern "C" fn(*const c_void, *const c_char) -> Ptr;

unsafe extern "C" {
    fn tessera_wpe_load(library: *const c_void, lookup: Lookup) -> *const c_char;
    fn tessera_wpe_new_display() -> Ptr;
    fn tessera_wpe_set_redraw(view: Ptr, callback: Option<unsafe extern "C" fn(Ptr)>, data: Ptr);
    fn tessera_wpe_place(view: Ptr, width: i32, height: i32, scale: f64);
    fn tessera_wpe_hide(view: Ptr);
    fn tessera_wpe_focus(view: Ptr, focused: i32);
    fn tessera_wpe_take_frame(
        view: Ptr,
        width: *mut i32,
        height: *mut i32,
        stride: *mut u32,
        pixels: *mut *const u8,
        length: *mut usize,
    ) -> Ptr;
    fn tessera_wpe_finish_frame(view: Ptr, buffer: Ptr, rendered: i32, released: i32);
    fn tessera_wpe_pointer(view: Ptr, phase: u32, mods: u32, x: f64, y: f64, dx: f64, dy: f64);
    fn tessera_wpe_button(view: Ptr, pressed: i32, mods: u32, button: u32, x: f64, y: f64);
    fn tessera_wpe_scroll(view: Ptr, mods: u32, x: f64, y: f64, dx: f64, dy: f64, precise: i32);
    fn tessera_wpe_touch(view: Ptr, phase: u32, mods: u32, id: u32, x: f64, y: f64);
    fn tessera_wpe_ime_snapshot(view: Ptr, snapshot: *mut WpeImeSnapshot) -> i32;
    fn tessera_wpe_ime_preedit(view: Ptr, text: *const c_char, cursor: u32);
    fn tessera_wpe_ime_commit(view: Ptr, text: *const c_char);
    fn tessera_wpe_ime_delete(view: Ptr, before_bytes: u32, after_bytes: u32);
    fn tessera_wpe_key_filtered(view: Ptr, pressed: i32, mods: u32, scan: u32, key: u32);
}

#[repr(C)]
struct WpeImeSnapshot {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    text: *const c_char,
    length: u32,
    cursor: u32,
    anchor: u32,
    password: i32,
    terminal: i32,
}

pub(super) unsafe fn load(library: &libloading::Library) -> Result<(), WebViewError> {
    unsafe extern "C" fn lookup(library: *const c_void, symbol: *const c_char) -> Ptr {
        unsafe {
            (&*library.cast::<libloading::Library>())
                .get::<Ptr>(CStr::from_ptr(symbol).to_bytes_with_nul())
                .map_or(null_mut(), |symbol| *symbol)
        }
    }
    let missing = unsafe { tessera_wpe_load(std::ptr::from_ref(library).cast(), lookup) };
    if !missing.is_null() {
        let name = unsafe { CStr::from_ptr(missing) }.to_string_lossy();
        return Err(error(format!("WPE Platform does not export {name}")));
    }
    Ok(())
}

pub(super) fn display() -> Result<Object, WebViewError> {
    let display = unsafe { tessera_wpe_new_display() };
    if display.is_null() {
        return Err(error("WPE Platform could not initialize its display"));
    }
    Ok(unsafe { from_glib_full(display.cast()) })
}

pub(super) struct Surface {
    view: NonNull<c_void>,
    surface: WaylandChildSurface,
    window: Arc<Window>,
    sink: EventSink,
    geometry: Cell<Option<(PxSize, f64)>>,
    size: Cell<Option<PxSize>>,
    next_id: Cell<u64>,
    retained: RefCell<HashMap<u64, SubmittedFrame>>,
    pointer: Cell<Option<PxPosition>>,
    buttons: Cell<u32>,
    touches: RefCell<HashMap<u64, (f64, f64)>>,
}

struct SubmittedFrame {
    buffer: NonNull<c_void>,
    rendered: bool,
}

impl Surface {
    pub(super) unsafe fn new(
        view: Ptr,
        surface: WaylandChildSurface,
        window: Arc<Window>,
        sink: EventSink,
    ) -> Self {
        unsafe extern "C" fn redraw(data: Ptr) {
            unsafe { (&*data.cast::<Window>()).request_redraw() };
        }
        unsafe {
            tessera_wpe_set_redraw(view, Some(redraw), Arc::as_ptr(&window).cast_mut().cast())
        };
        Self {
            view: NonNull::new(view).expect("live WPE view"),
            surface,
            window,
            sink,
            geometry: Cell::default(),
            size: Cell::default(),
            next_id: Cell::default(),
            retained: RefCell::default(),
            pointer: Cell::default(),
            buttons: Cell::default(),
            touches: RefCell::default(),
        }
    }

    pub(super) fn update(&self, surface: WaylandChildSurface, placement: WaylandViewPlacement) {
        let local = placement.local_size;
        let scale = placement.scale_factor;
        if self.geometry.replace(Some((local, scale))) != Some((local, scale)) {
            unsafe {
                tessera_wpe_place(
                    self.view.as_ptr(),
                    (f64::from(local.width.0) / scale).round().max(1.0) as i32,
                    (f64::from(local.height.0) / scale).round().max(1.0) as i32,
                    scale,
                )
            };
        }
        let (mut width, mut height, mut stride, mut pixels, mut length) = (0, 0, 0, null(), 0);
        let buffer = unsafe {
            tessera_wpe_take_frame(
                self.view.as_ptr(),
                &mut width,
                &mut height,
                &mut stride,
                &mut pixels,
                &mut length,
            )
        };
        let Some(buffer) = NonNull::new(buffer) else {
            return;
        };
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        let data = if pixels.is_null() || length > isize::MAX as usize {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(pixels, length) }
        };
        match surface.attach_shm_buffer(id, width.max(0) as u32, height.max(0) as u32, stride, data)
        {
            Ok(()) => {
                self.size.set(Some(PxSize::new(Px(width), Px(height))));
                self.retained.borrow_mut().insert(
                    id,
                    SubmittedFrame {
                        buffer,
                        rendered: false,
                    },
                );
            }
            Err(message) => {
                unsafe { tessera_wpe_finish_frame(self.view.as_ptr(), buffer.as_ptr(), 1, 1) };
                self.sink.event(WebViewEvent::Error {
                    error: error(message),
                });
            }
        }
    }

    pub(super) fn release(&self, id: u64) {
        if let Some(frame) = self.retained.borrow_mut().remove(&id) {
            unsafe {
                tessera_wpe_finish_frame(
                    self.view.as_ptr(),
                    frame.buffer.as_ptr(),
                    i32::from(!frame.rendered),
                    1,
                )
            };
        }
    }

    pub(super) fn rendered(&self, id: u64) {
        if let Some(frame) = self.retained.borrow_mut().get_mut(&id)
            && !std::mem::replace(&mut frame.rendered, true)
        {
            unsafe { tessera_wpe_finish_frame(self.view.as_ptr(), frame.buffer.as_ptr(), 1, 0) };
        }
    }

    pub(super) fn ime_request(&self) -> Option<ImeRequest> {
        let mut snapshot = WpeImeSnapshot {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            text: null(),
            length: 0,
            cursor: 0,
            anchor: 0,
            password: 0,
            terminal: 0,
        };
        if unsafe { tessera_wpe_ime_snapshot(self.view.as_ptr(), &mut snapshot) } == 0 {
            return None;
        }
        let scale = self.geometry.get()?.1;
        let text = if snapshot.text.is_null() || snapshot.length == 0 {
            String::new()
        } else {
            let bytes = unsafe {
                std::slice::from_raw_parts(snapshot.text.cast::<u8>(), snapshot.length as usize)
            };
            String::from_utf8_lossy(bytes).into_owned()
        };
        let length = text.len();
        let cursor = (snapshot.cursor as usize).min(length);
        let anchor = (snapshot.anchor as usize).min(length);
        let purpose = if snapshot.password != 0 {
            tessera_ui::winit::window::ImePurpose::Password
        } else if snapshot.terminal != 0 {
            tessera_ui::winit::window::ImePurpose::Terminal
        } else {
            tessera_ui::winit::window::ImePurpose::Normal
        };
        Some(
            ImeRequest::new(PxSize::new(
                Px((f64::from(snapshot.width) * scale).ceil() as i32),
                Px((f64::from(snapshot.height) * scale).ceil() as i32),
            ))
            .with_local_position(PxPosition::from_f32_xy(
                (f64::from(snapshot.x) * scale) as f32,
                (f64::from(snapshot.y) * scale) as f32,
            ))
            .with_text_state(tessera_ui::winit::event::ImeTextState {
                text,
                selection: anchor..cursor,
                composition: None,
            })
            .with_options(tessera_ui::winit::window::ImeOptions {
                purpose,
                ..Default::default()
            }),
        )
    }

    pub(super) fn ime_input(&self, event: tessera_ui::winit::event::Ime) {
        use tessera_ui::winit::event::Ime;

        match event {
            Ime::Preedit(text, cursor) => {
                let cursor = cursor.map_or(0, |(_, end)| {
                    let mut utf8_offset = 0;
                    text.chars()
                        .take_while(|ch| {
                            let inside = utf8_offset < end;
                            utf8_offset += ch.len_utf8();
                            inside
                        })
                        .map(char::len_utf16)
                        .sum::<usize>()
                });
                let text = std::ffi::CString::new(text.replace('\0', "\u{fffd}"))
                    .expect("NUL bytes are replaced");
                unsafe {
                    tessera_wpe_ime_preedit(self.view.as_ptr(), text.as_ptr(), cursor as u32)
                };
            }
            Ime::Commit(text) => {
                let text = std::ffi::CString::new(text.replace('\0', "\u{fffd}"))
                    .expect("NUL bytes are replaced");
                unsafe { tessera_wpe_ime_commit(self.view.as_ptr(), text.as_ptr()) };
            }
            Ime::DeleteSurroundingText { before, after } => unsafe {
                tessera_wpe_ime_delete(
                    self.view.as_ptr(),
                    before.min(u32::MAX as usize) as u32,
                    after.min(u32::MAX as usize) as u32,
                )
            },
            Ime::Enabled | Ime::Disabled | Ime::TextState(_) | Ime::Action(_) => {}
        }
        self.window.request_redraw();
    }

    pub(super) fn size(&self) -> Option<PxSize> {
        self.size.get()
    }

    pub(super) fn hide(&self) {
        self.size.set(None);
        self.geometry.set(None);
        unsafe { tessera_wpe_hide(self.view.as_ptr()) };
        // A hidden surface may not receive frame callbacks until it maps again.
        // Unblock the producer, but wait for release before returning each buffer.
        for frame in self.retained.borrow_mut().values_mut() {
            if !std::mem::replace(&mut frame.rendered, true) {
                unsafe {
                    tessera_wpe_finish_frame(self.view.as_ptr(), frame.buffer.as_ptr(), 1, 0)
                };
            }
        }
    }

    pub(super) fn request_focus(&self) {
        self.surface.request_focus();
        self.window.request_redraw();
    }

    fn focus(&self, focused: bool) {
        unsafe { tessera_wpe_focus(self.view.as_ptr(), i32::from(focused)) };
    }

    pub(super) fn input(&self, event: LinuxViewInputEvent, modifiers: ModifiersState) -> bool {
        let view = self.view.as_ptr();
        let scale = self.geometry.get().map_or(1.0, |(_, scale)| scale);
        let mods = u32::from(modifiers.control_key())
            | (u32::from(modifiers.shift_key()) << 1)
            | (u32::from(modifiers.alt_key()) << 2)
            | (u32::from(modifiers.super_key()) << 3);
        let point = |p: PxPosition| (f64::from(p.x) / scale, f64::from(p.y) / scale);
        let active = || mods | self.buttons.get();
        unsafe {
            match event {
                LinuxViewInputEvent::Focused(value) => self.focus(value),
                LinuxViewInputEvent::Move(pos) => {
                    let (x, y) = point(pos);
                    if let Some(old) = self.pointer.replace(Some(pos)) {
                        let (ox, oy) = point(old);
                        tessera_wpe_pointer(view, 0, active(), x, y, x - ox, y - oy);
                    } else {
                        tessera_wpe_pointer(view, 1, active(), x, y, 0.0, 0.0);
                    }
                }
                LinuxViewInputEvent::Button {
                    position,
                    button,
                    state,
                } => {
                    let (button, index) = match button {
                        MouseButton::Left => (1, 1),
                        MouseButton::Middle => (2, 2),
                        MouseButton::Right => (3, 3),
                        MouseButton::Back => (7, 4),
                        MouseButton::Forward => (6, 5),
                        MouseButton::Other(_) => return false,
                    };
                    let pressed = state == ElementState::Pressed;
                    let mask = 1 << (index + 7);
                    let old = self.buttons.get();
                    self.buttons
                        .set(if pressed { old | mask } else { old & !mask });
                    let (x, y) = point(position);
                    tessera_wpe_button(view, i32::from(pressed), active(), button, x, y);
                }
                LinuxViewInputEvent::Wheel { position, delta } => {
                    let (x, y) = point(position);
                    let (dx, dy, precise) = match delta {
                        MouseScrollDelta::LineDelta(dx, dy) => (f64::from(dx), f64::from(dy), 0),
                        MouseScrollDelta::PixelDelta(p) => (p.x / scale, p.y / scale, 1),
                    };
                    tessera_wpe_scroll(view, active(), x, y, dx, dy, precise);
                }
                LinuxViewInputEvent::Key(event) => {
                    let Some(key) = keysym(&event.logical_key) else {
                        return false;
                    };
                    let scan = event
                        .physical_key
                        .to_scancode()
                        .map_or(0, |scan| scan.saturating_add(8));
                    tessera_wpe_key_filtered(
                        view,
                        i32::from(event.state == ElementState::Pressed),
                        active(),
                        scan,
                        key,
                    );
                }
                LinuxViewInputEvent::Ime(event) => self.ime_input(event),
                LinuxViewInputEvent::Touch(touch) => {
                    let phase = match touch.phase {
                        TouchPhase::Started => 0,
                        TouchPhase::Moved => 1,
                        TouchPhase::Ended => 2,
                        TouchPhase::Cancelled => 3,
                    };
                    let (x, y) = (touch.location.x / scale, touch.location.y / scale);
                    if phase < 2 {
                        self.touches.borrow_mut().insert(touch.id, (x, y));
                    } else {
                        self.touches.borrow_mut().remove(&touch.id);
                    }
                    tessera_wpe_touch(view, phase, active(), touch.id as u32, x, y);
                }
                LinuxViewInputEvent::Leave | LinuxViewInputEvent::Cancel => {
                    if let Some(pos) = self.pointer.take() {
                        let (x, y) = point(pos);
                        tessera_wpe_pointer(view, 2, active(), x, y, 0.0, 0.0);
                    }
                    if matches!(event, LinuxViewInputEvent::Cancel) {
                        let buttons = self.buttons.take();
                        for (button, index) in [(1, 1), (2, 2), (3, 3), (7, 4), (6, 5)] {
                            if buttons & (1 << (index + 7)) != 0 {
                                tessera_wpe_button(view, 0, mods, button, -1.0, -1.0);
                            }
                        }
                        for (id, (x, y)) in self.touches.borrow_mut().drain() {
                            tessera_wpe_touch(view, 3, mods, id as u32, x, y);
                        }
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

fn keysym(key: &Key) -> Option<u32> {
    let name = match key {
        Key::Character(text) => {
            return text
                .chars()
                .next()
                .map(|ch| unsafe { gtk::gdk::ffi::gdk_unicode_to_keyval(ch as u32) });
        }
        Key::Named(NamedKey::Enter) => c"Return",
        Key::Named(NamedKey::Tab) => c"Tab",
        Key::Named(NamedKey::Space) => c"space",
        Key::Named(NamedKey::Escape) => c"Escape",
        Key::Named(NamedKey::Backspace) => c"BackSpace",
        Key::Named(NamedKey::Delete) => c"Delete",
        Key::Named(NamedKey::Home) => c"Home",
        Key::Named(NamedKey::End) => c"End",
        Key::Named(NamedKey::PageUp) => c"Page_Up",
        Key::Named(NamedKey::PageDown) => c"Page_Down",
        Key::Named(NamedKey::ArrowLeft) => c"Left",
        Key::Named(NamedKey::ArrowRight) => c"Right",
        Key::Named(NamedKey::ArrowUp) => c"Up",
        Key::Named(NamedKey::ArrowDown) => c"Down",
        Key::Named(NamedKey::Shift) => c"Shift_L",
        Key::Named(NamedKey::Control) => c"Control_L",
        Key::Named(NamedKey::Alt) => c"Alt_L",
        Key::Named(NamedKey::Super) => c"Super_L",
        _ => return None,
    };
    Some(unsafe { gtk::gdk::ffi::gdk_keyval_from_name(name.as_ptr()) })
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { tessera_wpe_set_redraw(self.view.as_ptr(), None, null_mut()) };
        self.hide();
        for (_, frame) in self.retained.get_mut().drain() {
            unsafe {
                tessera_wpe_finish_frame(
                    self.view.as_ptr(),
                    frame.buffer.as_ptr(),
                    i32::from(!frame.rendered),
                    1,
                )
            };
        }
    }
}
