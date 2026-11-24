#include <wpe/wpe-platform.h>

#define WPE_FUNCTIONS(F) \
    F(wpe_display_get_type) \
    F(wpe_display_connect) \
    F(wpe_display_set_primary) \
    F(wpe_display_set_available_input_devices) \
    F(wpe_toplevel_get_type) \
    F(wpe_toplevel_resized) \
    F(wpe_toplevel_scale_changed) \
    F(wpe_toplevel_state_changed) \
    F(wpe_view_get_type) \
    F(wpe_view_get_toplevel) \
    F(wpe_view_set_toplevel) \
    F(wpe_view_get_display) \
    F(wpe_view_compute_press_count) \
    F(wpe_view_resized) \
    F(wpe_view_map) \
    F(wpe_view_unmap) \
    F(wpe_view_focus_in) \
    F(wpe_view_focus_out) \
    F(wpe_view_error_quark) \
    F(wpe_view_buffer_rendered) \
    F(wpe_view_buffer_released) \
    F(wpe_input_method_context_get_type) \
    F(wpe_input_method_context_get_view) \
    F(wpe_input_method_context_get_input_purpose) \
    F(wpe_input_method_context_filter_key_event) \
    F(wpe_buffer_get_width) \
    F(wpe_buffer_get_height) \
    F(wpe_buffer_shm_get_type) \
    F(wpe_buffer_shm_get_data) \
    F(wpe_buffer_shm_get_stride) \
    F(wpe_view_event) \
    F(wpe_event_unref) \
    F(wpe_event_pointer_move_new) \
    F(wpe_event_pointer_button_new) \
    F(wpe_event_scroll_new) \
    F(wpe_event_keyboard_new) \
    F(wpe_event_touch_new)

#define DECLARE(name) __typeof__(&name) name;
static struct { WPE_FUNCTIONS(DECLARE) } api;
#undef DECLARE

typedef struct {
    WPEDisplay parent;
} TesseraWpeDisplay;

typedef struct {
    WPEToplevel parent;
} TesseraWpeToplevel;

typedef struct {
    int x, y, width, height;
    const char *text;
    guint length, cursor, anchor;
    gboolean password, terminal;
} TesseraWpeImeSnapshot;

typedef struct {
    WPEInputMethodContext parent;
    gboolean focused;
    gchar *surrounding;
    guint length, cursor, anchor;
    WPERectangle area;
    gchar *preedit;
    guint preedit_cursor;
} TesseraWpeIme;

typedef struct {
    WPEView parent;
    WPEBuffer *pending;
    TesseraWpeIme *ime;
    void (*request_redraw)(void *);
    void *redraw_data;
} TesseraWpeView;

typedef WPEDisplayClass TesseraWpeDisplayClass;
typedef WPEToplevelClass TesseraWpeToplevelClass;
typedef WPEViewClass TesseraWpeViewClass;
typedef WPEInputMethodContextClass TesseraWpeImeClass;

G_DEFINE_TYPE(TesseraWpeToplevel, tessera_wpe_toplevel, api.wpe_toplevel_get_type())
G_DEFINE_TYPE(TesseraWpeView, tessera_wpe_view, api.wpe_view_get_type())
G_DEFINE_TYPE(TesseraWpeDisplay, tessera_wpe_display, api.wpe_display_get_type())
G_DEFINE_TYPE(TesseraWpeIme, tessera_wpe_ime, api.wpe_input_method_context_get_type())

static void tessera_wpe_toplevel_init(TesseraWpeToplevel *toplevel) { (void)toplevel; }
static void tessera_wpe_toplevel_class_init(TesseraWpeToplevelClass *klass) { (void)klass; }
static void tessera_wpe_view_init(TesseraWpeView *view) { (void)view; }
static void tessera_wpe_display_init(TesseraWpeDisplay *display) { (void)display; }
static void tessera_wpe_ime_init(TesseraWpeIme *ime) { (void)ime; }

static void ime_request_redraw(WPEInputMethodContext *context)
{
    TesseraWpeView *view = (TesseraWpeView *)api.wpe_input_method_context_get_view(context);
    if (view && view->request_redraw)
        view->request_redraw(view->redraw_data);
}

static void ime_purpose_changed(WPEInputMethodContext *context, GParamSpec *spec, gpointer data)
{
    (void)spec;
    (void)data;
    ime_request_redraw(context);
}

static void ime_reset(WPEInputMethodContext *context)
{
    TesseraWpeIme *ime = (TesseraWpeIme *)context;
    g_clear_pointer(&ime->preedit, g_free);
    ime->preedit_cursor = 0;
}

static void ime_finalize(GObject *object)
{
    TesseraWpeIme *ime = (TesseraWpeIme *)object;
    g_free(ime->surrounding);
    g_free(ime->preedit);
    G_OBJECT_CLASS(tessera_wpe_ime_parent_class)->finalize(object);
}

static void ime_focus_in(WPEInputMethodContext *context)
{
    ((TesseraWpeIme *)context)->focused = TRUE;
    ime_request_redraw(context);
}

static void ime_focus_out(WPEInputMethodContext *context)
{
    TesseraWpeIme *ime = (TesseraWpeIme *)context;
    ime->focused = FALSE;
    g_clear_pointer(&ime->surrounding, g_free);
    ime->length = ime->cursor = ime->anchor = 0;
    ime->area = (WPERectangle){0};
    ime_reset(context);
    ime_request_redraw(context);
}

static void ime_cursor_area(WPEInputMethodContext *context, int x, int y, int width, int height)
{
    ((TesseraWpeIme *)context)->area = (WPERectangle){x, y, width, height};
    ime_request_redraw(context);
}

static void ime_surrounding(WPEInputMethodContext *context, const char *text, guint length,
                            guint cursor, guint anchor)
{
    TesseraWpeIme *ime = (TesseraWpeIme *)context;
    g_free(ime->surrounding);
    ime->surrounding = g_strndup(text, length);
    ime->length = ime->surrounding ? (guint)strlen(ime->surrounding) : 0;
    ime->cursor = MIN(cursor, ime->length);
    ime->anchor = MIN(anchor, ime->length);
    ime_request_redraw(context);
}

static guint utf8_count_bytes(const char *text, guint length, guint start, guint end)
{
    start = MIN(start, length);
    end = MIN(end, length);
    while (start < end && (((guint8)text[start] & 0xc0) == 0x80))
        start++;
    while (end > start && end < length && (((guint8)text[end] & 0xc0) == 0x80))
        end--;
    return (guint)g_utf8_strlen(text + start, end - start);
}

static void ime_get_preedit(WPEInputMethodContext *context, char **text, GList **underlines,
                            guint *cursor)
{
    TesseraWpeIme *ime = (TesseraWpeIme *)context;
    if (text)
        *text = g_strdup(ime->preedit ? ime->preedit : "");
    if (underlines)
        *underlines = NULL;
    if (cursor)
        *cursor = ime->preedit_cursor;
}

static void tessera_wpe_ime_class_init(TesseraWpeImeClass *klass)
{
    G_OBJECT_CLASS(klass)->finalize = ime_finalize;
    klass->focus_in = ime_focus_in;
    klass->focus_out = ime_focus_out;
    klass->set_cursor_area = ime_cursor_area;
    klass->set_surrounding = ime_surrounding;
    klass->get_preedit_string = ime_get_preedit;
    klass->reset = ime_reset;
}

static WPEInputMethodContext *create_input_method_context(WPEDisplay *display, WPEView *base)
{
    (void)display;
    TesseraWpeView *view = (TesseraWpeView *)base;
    if (view->ime)
        return g_object_ref((WPEInputMethodContext *)view->ime);
    view->ime = g_object_new(tessera_wpe_ime_get_type(), "view", base, NULL);
    g_object_add_weak_pointer(G_OBJECT(view->ime), (gpointer *)&view->ime);
    g_signal_connect(view->ime, "notify::input-purpose", G_CALLBACK(ime_purpose_changed), NULL);
    return (WPEInputMethodContext *)view->ime;
}

gboolean tessera_wpe_ime_snapshot(WPEView *base, TesseraWpeImeSnapshot *snapshot)
{
    TesseraWpeIme *ime = ((TesseraWpeView *)base)->ime;
    if (!ime || !ime->focused)
        return FALSE;
    WPEInputPurpose purpose = api.wpe_input_method_context_get_input_purpose((WPEInputMethodContext *)ime);
    gboolean password = purpose == WPE_INPUT_PURPOSE_PASSWORD || purpose == WPE_INPUT_PURPOSE_PIN;
    *snapshot = (TesseraWpeImeSnapshot){ime->area.x, ime->area.y, ime->area.width, ime->area.height,
        password ? NULL : ime->surrounding, password ? 0 : ime->length,
        password ? 0 : ime->cursor, password ? 0 : ime->anchor, password,
        purpose == WPE_INPUT_PURPOSE_TERMINAL};
    return TRUE;
}

void tessera_wpe_ime_preedit(WPEView *base, const char *text, guint cursor)
{
    TesseraWpeIme *ime = ((TesseraWpeView *)base)->ime;
    if (!ime || !ime->focused)
        return;
    if (!*text) {
        if (ime->preedit) {
            ime_reset((WPEInputMethodContext *)ime);
            g_signal_emit_by_name(ime, "preedit-finished");
        }
        return;
    }
    if (!ime->preedit)
        g_signal_emit_by_name(ime, "preedit-started");
    g_free(ime->preedit);
    ime->preedit = g_strdup(text);
    ime->preedit_cursor = cursor;
    g_signal_emit_by_name(ime, "preedit-changed");
}

void tessera_wpe_ime_commit(WPEView *base, const char *text)
{
    TesseraWpeIme *ime = ((TesseraWpeView *)base)->ime;
    if (ime && ime->focused) {
        ime_reset((WPEInputMethodContext *)ime);
        g_signal_emit_by_name(ime, "committed", text);
    }
}

void tessera_wpe_ime_delete(WPEView *base, guint before_bytes, guint after_bytes)
{
    TesseraWpeIme *ime = ((TesseraWpeView *)base)->ime;
    if (!ime || !ime->focused)
        return;
    guint before = 0, after = 0;
    if (ime->surrounding) {
        before = utf8_count_bytes(ime->surrounding, ime->length,
            ime->cursor > before_bytes ? ime->cursor - before_bytes : 0, ime->cursor);
        guint after_end = after_bytes > ime->length - ime->cursor
            ? ime->length : ime->cursor + after_bytes;
        after = utf8_count_bytes(ime->surrounding, ime->length, ime->cursor,
            after_end);
    }
    g_signal_emit_by_name(ime, "delete-surrounding", -(gint)before, before + after);
}

void tessera_wpe_finish_frame(WPEView *view, WPEBuffer *buffer, gboolean rendered, gboolean released)
{
    if (rendered)
        api.wpe_view_buffer_rendered(view, buffer);
    if (released) {
        api.wpe_view_buffer_released(view, buffer);
        g_object_unref(buffer);
    }
}

static void discard_pending(TesseraWpeView *view)
{
    WPEBuffer *buffer = g_steal_pointer(&view->pending);
    if (buffer)
        tessera_wpe_finish_frame((WPEView *)view, buffer, TRUE, TRUE);
}

static gboolean render_buffer(WPEView *base, WPEBuffer *buffer, const WPERectangle *damage,
                              guint count, GError **error)
{
    (void)damage;
    (void)count;
    TesseraWpeView *view = (TesseraWpeView *)base;
    if (!G_TYPE_CHECK_INSTANCE_TYPE(buffer, api.wpe_buffer_shm_get_type())) {
        g_set_error_literal(error, api.wpe_view_error_quark(), WPE_VIEW_ERROR_RENDER_FAILED,
                            "The compositor requires a shared-memory WebKit buffer");
        return FALSE;
    }
    discard_pending(view);
    view->pending = g_object_ref(buffer);
    if (view->request_redraw)
        view->request_redraw(view->redraw_data);
    return TRUE;
}

static void view_dispose(GObject *object)
{
    TesseraWpeView *view = (TesseraWpeView *)object;
    view->request_redraw = NULL;
    if (view->ime) {
        g_object_remove_weak_pointer(G_OBJECT(view->ime), (gpointer *)&view->ime);
        view->ime = NULL;
    }
    g_clear_object(&view->pending);
    G_OBJECT_CLASS(tessera_wpe_view_parent_class)->dispose(object);
}

static void tessera_wpe_view_class_init(TesseraWpeViewClass *klass)
{
    G_OBJECT_CLASS(klass)->dispose = view_dispose;
    klass->render_buffer = render_buffer;
}

static gboolean connect_display(WPEDisplay *display, GError **error)
{
    (void)display;
    (void)error;
    return TRUE;
}

static WPEView *create_view(WPEDisplay *display)
{
    return g_object_new(tessera_wpe_view_get_type(), "display", display, NULL);
}

static WPEToplevel *create_toplevel(WPEDisplay *display, guint max_views)
{
    return g_object_new(tessera_wpe_toplevel_get_type(), "display", display,
                        "max-views", max_views, NULL);
}

static void tessera_wpe_display_class_init(TesseraWpeDisplayClass *klass)
{
    klass->connect = connect_display;
    klass->create_view = create_view;
    klass->create_toplevel = create_toplevel;
    klass->create_input_method_context = create_input_method_context;
}

const char *tessera_wpe_load(const void *library, void *(*lookup)(const void *, const char *))
{
#define LOAD(name) if (!(api.name = (__typeof__(api.name))lookup(library, #name))) return #name;
    WPE_FUNCTIONS(LOAD)
#undef LOAD
    return NULL;
}

WPEDisplay *tessera_wpe_new_display(void)
{
    WPEDisplay *display = g_object_new(tessera_wpe_display_get_type(), NULL);
    api.wpe_display_set_available_input_devices(display, WPE_AVAILABLE_INPUT_DEVICE_MOUSE |
        WPE_AVAILABLE_INPUT_DEVICE_KEYBOARD | WPE_AVAILABLE_INPUT_DEVICE_TOUCHSCREEN);
    if (!api.wpe_display_connect(display, NULL)) {
        g_object_unref(display);
        return NULL;
    }
    api.wpe_display_set_primary(display);
    return display;
}

void tessera_wpe_set_redraw(WPEView *base, void (*callback)(void *), void *data)
{
    TesseraWpeView *view = (TesseraWpeView *)base;
    view->request_redraw = callback;
    view->redraw_data = data;
}

void tessera_wpe_place(WPEView *view, int width, int height, double scale)
{
    WPEToplevel *toplevel = api.wpe_view_get_toplevel(view);
    if (!toplevel) {
        toplevel = create_toplevel(api.wpe_view_get_display(view), 1);
        api.wpe_view_set_toplevel(view, toplevel);
        g_object_unref(toplevel);
    }
    api.wpe_toplevel_scale_changed(toplevel, scale);
    api.wpe_toplevel_resized(toplevel, width, height);
    api.wpe_view_resized(view, width, height);
    api.wpe_view_map(view);
}

void tessera_wpe_hide(WPEView *view)
{
    api.wpe_view_unmap(view);
    discard_pending((TesseraWpeView *)view);
}

void tessera_wpe_focus(WPEView *view, gboolean focused)
{
    WPEToplevel *toplevel = api.wpe_view_get_toplevel(view);
    if (toplevel)
        api.wpe_toplevel_state_changed(toplevel,
            focused ? WPE_TOPLEVEL_STATE_ACTIVE : WPE_TOPLEVEL_STATE_NONE);
    if (focused)
        api.wpe_view_focus_in(view);
    else
        api.wpe_view_focus_out(view);
}

static void send_event(WPEView *view, WPEEvent *event)
{
    api.wpe_view_event(view, event);
    api.wpe_event_unref(event);
}

static guint32 event_time(void) { return (guint32)(g_get_monotonic_time() / 1000); }

void tessera_wpe_pointer(WPEView *view, guint phase, guint modifiers, double x, double y,
                         double dx, double dy)
{
    const WPEEventType types[] = {WPE_EVENT_POINTER_MOVE, WPE_EVENT_POINTER_ENTER, WPE_EVENT_POINTER_LEAVE};
    send_event(view, api.wpe_event_pointer_move_new(types[phase], view, WPE_INPUT_SOURCE_MOUSE,
        event_time(), modifiers, x, y, dx, dy));
}

void tessera_wpe_button(WPEView *view, gboolean pressed, guint modifiers, guint button,
                        double x, double y)
{
    guint32 time = event_time();
    guint count = pressed ? api.wpe_view_compute_press_count(view, x, y, button, time) : 0;
    send_event(view, api.wpe_event_pointer_button_new(pressed ? WPE_EVENT_POINTER_DOWN : WPE_EVENT_POINTER_UP,
        view, WPE_INPUT_SOURCE_MOUSE, time, modifiers, button, x, y, count));
}

void tessera_wpe_scroll(WPEView *view, guint modifiers, double x, double y,
                        double dx, double dy, gboolean precise)
{
    send_event(view, api.wpe_event_scroll_new(view, precise ? WPE_INPUT_SOURCE_TOUCHPAD : WPE_INPUT_SOURCE_MOUSE,
        event_time(), modifiers, dx, dy, precise, FALSE, x, y));
}

void tessera_wpe_touch(WPEView *view, guint phase, guint modifiers, guint id, double x, double y)
{
    const WPEEventType types[] = {WPE_EVENT_TOUCH_DOWN, WPE_EVENT_TOUCH_MOVE, WPE_EVENT_TOUCH_UP, WPE_EVENT_TOUCH_CANCEL};
    send_event(view, api.wpe_event_touch_new(types[phase], view, WPE_INPUT_SOURCE_TOUCHSCREEN,
        event_time(), modifiers, id, x, y));
}

void tessera_wpe_key_filtered(WPEView *view, gboolean pressed, guint modifiers,
                              guint keycode, guint keysym)
{
    WPEEvent *event = api.wpe_event_keyboard_new(
        pressed ? WPE_EVENT_KEYBOARD_KEY_DOWN : WPE_EVENT_KEYBOARD_KEY_UP,
        view, WPE_INPUT_SOURCE_KEYBOARD, event_time(), modifiers, keycode, keysym);
    TesseraWpeView *wpe_view = (TesseraWpeView *)view;
    if (!wpe_view->ime || !api.wpe_input_method_context_filter_key_event(
            (WPEInputMethodContext *)wpe_view->ime, event))
        api.wpe_view_event(view, event);
    api.wpe_event_unref(event);
}

WPEBuffer *tessera_wpe_take_frame(WPEView *base, int *width, int *height, unsigned *stride,
                                 const void **pixels, size_t *length)
{
    TesseraWpeView *view = (TesseraWpeView *)base;
    WPEBuffer *buffer = g_steal_pointer(&view->pending);
    if (!buffer)
        return NULL;
    *width = api.wpe_buffer_get_width(buffer);
    *height = api.wpe_buffer_get_height(buffer);
    *stride = api.wpe_buffer_shm_get_stride((WPEBufferSHM *)buffer);
    *pixels = g_bytes_get_data(api.wpe_buffer_shm_get_data((WPEBufferSHM *)buffer), length);
    return buffer;
}
