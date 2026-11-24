# tessera-webview

Native WebView components for Tessera apps.

## Install

```sh
cargo add tessera-webview
```

## Usage

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
| Windows | WebView2 with DirectComposition |
| macOS | AppKit `WKWebView` |
| Linux (X11 / XWayland) | WebKitGTK in a GTK 3 child window |
| Linux (Wayland) | WPE WebKit 2.54+ |
| Android 11+ | AndroidX WebView |
| iOS | `WKWebView` |
