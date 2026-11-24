use tessera_ui::{EntryPoint, remember, tessera};
use tessera_webview::{
    IpcPolicy, WebViewController, WebViewEvent, WebViewOptions, WebViewPackage, WebViewSource,
    webview,
};

const HTML: &str = r#"<!doctype html><html><meta charset="utf-8"><title>Tessera WebView</title>
<style>body{font:20px system-ui;padding:40px;color:#243047;background:#f4f7fb}button{font:inherit;padding:12px 20px;border-radius:8px;border:1px solid #8da0b8;cursor:pointer}code{font-size:16px}</style>
<h1>Native browser. Tessera layout.</h1><p>This document is running inside your platform's WebView.</p>
<button onclick="window.ipc.postMessage('Hello from JavaScript!')">Send a message to Rust</button>
<p id="reply">Click the button to test the bridge.</p></html>"#;

#[tessera]
fn app() {
    let controller = remember(WebViewController::new).get();
    webview(WebViewSource::html(HTML))
        .controller(controller)
        .options(WebViewOptions { ipc: IpcPolicy::AllowAll, ..Default::default() })
        .on_event(move |event: WebViewEvent| {
            println!("{event:?}");
            if matches!(event, WebViewEvent::Message { .. }) {
                controller.evaluate_script("document.getElementById('reply').textContent = 'Rust received your message.'").unwrap();
            }
        });
}

#[tessera_ui::entry]
fn entry() -> EntryPoint {
    EntryPoint::new(app).package(WebViewPackage)
}

fn main() {
    #[cfg(not(any(target_os = "android", target_os = "ios", target_family = "wasm")))]
    entry().run_desktop().unwrap();
    #[cfg(target_os = "ios")]
    entry().run_ios().unwrap();
}
