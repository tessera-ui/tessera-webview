# tessera-webview

A Rust library that provides native WebView components for Tessera applications.

## Add to your project

```sh
cargo add tessera-webview
```

## Example

```rust
use tessera_ui::tessera;
use tessera_webview::{WebViewSource, webview};

#[tessera]
fn app() {
    webview(WebViewSource::url("https://example.com"));
}
```

## Supported platforms

| Platform | Backend |
| --- | --- |
| Windows | Wry (WebView2) |
| macOS | Wry (WKWebView) |
| Linux (X11 / XWayland) | Wry (WebKitGTK) |
| Android 11+ (API 30+) | `android.webkit.WebView` (AndroidX WebKit) |
| iOS | `WKWebView` |
