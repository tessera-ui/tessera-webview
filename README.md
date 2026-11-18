# tessera-webview

Native browser views in Tessera layouts, with navigation, JavaScript, optional
IPC, and component-owned cleanup. Built against Tessera UI 3.0 and Wry 0.57.

## Try it locally

```sh
cargo run --example browser
```

The example opens an HTML page with a button that sends a message to Rust and
receives a reply. Windows needs the Microsoft Edge WebView2 Runtime.

For an application alongside this checkout:

```toml
[dependencies]
tessera-ui = "3.0.0"
tessera-webview = { path = "../tessera-webview" }
```

```rust
use tessera_ui::{EntryPoint, remember, tessera};
use tessera_webview::{
    EntryPointWebViewExt, WebViewController, WebViewSource, webview,
};

#[tessera]
fn app() {
    let browser = remember(WebViewController::new).get();
    webview(WebViewSource::url("https://example.com"))
        .controller(browser)
        .on_event(|event| println!("{event:?}"));
}

#[tessera_ui::entry]
fn entry() -> EntryPoint {
    EntryPoint::new(app).webview()
}

fn main() {
    entry().run_desktop().unwrap();
}
```

The component fills its available space unless given `.modifier(...)`. A
controller is optional. Remember an external controller rather than allocating
one on every rebuild; each controller may belong to only one mounted component.
Wait for `WebViewEvent::Ready` before sending commands.

## API and lifetime

- Sources: `WebViewSource::url(...)` and `WebViewSource::html(...)`.
- Navigation: `load_url`, `load_html`, `go_back`, `go_forward`, `reload`.
- Queries: `is_ready`, `url`, `can_go_back`, `can_go_forward`, `bounds`.
- JavaScript: `evaluate_script` schedules execution; `evaluate(...).await`
  returns a JSON-encoded result, not a resolved JavaScript Promise.
- Other commands: `focus`, `set_zoom`, `clear_browsing_data`.
- Options: navigation/IPC policies, initialization scripts, user agent,
  JavaScript, devtools, incognito, and RGBA background.
- Events: ready, load start/finish, title, message, blocked navigation, popup,
  download, and engine/configuration errors.

Run controllers and evaluation futures on their owning Tessera UI session, for
example in an event callback or retained `spawn_local` task. Evaluations time out
after ten seconds and are cancelled on unmount, suspension, or recreation.
Engine script exceptions can produce `null`; wrap scripts when an application
needs structured error reporting.

Changing the declarative source navigates even when layout is unchanged.
Imperative controller navigation is not undone by ordinary rebuilds or resizing.
Changing options recreates the browser, losing history and document state.
Suspension releases native resources; resume reloads the last app-requested
source. In-page navigation, history, scroll position, and form state are not
restored. Removing the component destroys its browser automatically.

Android commands are queued on the Activity UI thread. Successful return means
the command was queued; later failures arrive as events. Its synchronous query
methods return the most recent UI-thread snapshot.

## Security

IPC is **disabled by default**. To receive messages from a particular origin:

```rust
use tessera_webview::{IpcPolicy, NavigationPolicy, WebViewOptions};

let options = WebViewOptions {
    navigation: NavigationPolicy::Origins(vec!["https://example.com".into()]),
    ipc: IpcPolicy::Origins(vec!["https://example.com".into()]),
    ..Default::default()
};
```

The permitted page calls `window.ipc.postMessage("message")`; Rust receives
`WebViewEvent::Message { url, body }`. Origins are parsed and compared by scheme,
host, and effective port, not string prefix. Messages over one MiB are discarded.
Treat message bodies as untrusted input and validate the requested operation.
`IpcPolicy::AllowAll` also permits opaque inline HTML and must only be used with
trusted documents and trusted embedded frames.

Linux origin-allowlisted IPC is rejected: Wry's WebKitGTK backend reports the
top-level URL for iframe messages, which cannot establish the sender's origin.
Use `Disabled`, or explicitly choose `AllowAll` for fully trusted content.
Android requires origin-verifying AndroidX WebKit message support and accepts
messages from the main frame only; there is no unsafe legacy bridge fallback.

External URL schemes, credential-bearing URLs, popups, permission requests, and
automatic downloads are denied. `about:blank` remains available for inline HTML.
The Windows backend allows only the exact app-supplied HTML data navigation, not
arbitrary page-generated `data:` URLs. HTTP(S) navigation is unrestricted unless
an origin allowlist is provided. These policies are **not a network sandbox**:
subresources, redirects, scripts, forms, and engine-specific behavior still
need an appropriate server-side CSP and application threat model. Do not embed
untrusted HTML by string interpolation. Persistent stores can be shared, and
clearing browsing data can affect other browser instances.
Permission decisions already persisted in an engine profile may bypass its
request callback; use an ephemeral profile where supported when isolation is
required. Browser event queues are bounded, so excessive page messages can drop
events rather than grow native memory without limit.

## Platforms and prerequisites

| Target | Backend | Requirements |
| --- | --- | --- |
| Windows | WebView2 | Microsoft Edge WebView2 Runtime |
| Android | `android.webkit.WebView`, AndroidX WebKit | Activity host; see below |
| macOS / iOS | WKWebView | Xcode and target SDK |
| Linux X11 | WebKitGTK | GTK 3 and WebKitGTK 4.1 |
| Linux native Wayland | Unsupported | Use an X11/XWayland Tessera window |
| Browser/Wasm and other targets | No native backend | Reports unsupported platform |

Linux needs GTK 3 and WebKitGTK 4.1 development packages, for example
`libgtk-3-dev libwebkit2gtk-4.1-dev` on Debian/Ubuntu, plus Tessera's native build
dependencies. Apple targets need Xcode and the target SDK; use macOS 11+/iOS 14+
for the exposed page-zoom operation. Use HTTPS or configure the application's
Apple transport-security policy deliberately. Windows incognito mode requires
WebView2 Runtime 101 or newer.

Under a Wayland desktop, select X11 for both hosts when using XWayland, for
example `GDK_BACKEND=x11 WINIT_UNIX_BACKEND=x11 cargo run --example browser`.

The Android library is in `android/`; `cargo tessera` discovers it through the
crate's Tessera package metadata. It uses compile SDK 35, min SDK 24, Java 17,
and AndroidX WebKit 1.14. The standalone Gradle settings use AGP 8.10.1
(Gradle 8.11.1+). Set `ANDROID_HOME`, `JAVA_HOME`, and `android.useAndroidX=true`
in the host app. The library contributes the Internet permission. Cleartext
HTTP still requires an app-level network security configuration. Per-view
incognito and devtools are rejected on Android rather than changing global
process settings. Initialization scripts use document-start support where
available and otherwise run at page start. Android popup events have an empty
URL because the engine does not expose it before a popup is accepted.

## Native-surface limitations

Native browsers sit **above** Tessera's GPU scene. They do not participate in
GPU opacity, transforms, rounded clips, screenshots, or overlay ordering. Use
non-overlapping, axis-aligned rectangular regions; do not place a Tessera popup
over a browser. Partially clipped or offscreen browsers are hidden rather than
leaking pixels/input outside their viewport. A zero-sized or initially clipped
view is not created until it becomes fully visible. Clipped views can therefore
remain `NotReady`. Bounds use physical window pixels and are converted to native
logical units where needed. This is not an offscreen browser renderer.

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo tessera check --all-targets
cargo run --example smoke
```

The smoke example requires a desktop/GPU and cannot run headlessly or on Linux,
where origin-verified IPC is unsupported. Runtime validation so far is Windows
only; Android has build validation, while Apple and Linux targets remain unverified.

Android checks, with the NDK C/C++ toolchain configured for the Rust target:

```sh
cargo check --lib --target aarch64-linux-android
gradle -p android assembleDebug
```
