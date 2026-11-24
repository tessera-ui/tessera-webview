# tessera-webview

A Rust library that provides native WebView components for Tessera applications.

## Add to your project

```sh
cargo add tessera-webview
```

## Example

```rust
use tessera_ui::{EntryPoint, tessera};
use tessera_webview::{EntryPointWebViewExt, WebViewSource, webview};

#[tessera]
fn app() {
    webview(WebViewSource::url("https://example.com"));
}

fn main() {
    EntryPoint::new(app).webview().run_desktop().unwrap();
}
```

## Supported platforms

| Platform | Backend |
| --- | --- |
| Windows | WebView2 with DirectComposition; requires the WebView2 Runtime |
| macOS | `WKWebView` in AppKit |
| Linux (X11 / XWayland) | WebKitGTK (`webkit2gtk-4.1`) in a GTK 3 child window |
| Linux (Wayland) | WPE WebKit with WPE Platform 2.54+; uses a `wl_subsurface` and `wl_shm` |
| Android 11+ (API 30+) | `android.webkit.WebView` (AndroidX WebKit) |
| iOS | `WKWebView` |
