//! Exercises native rendering, text editing, scrolling, and IPC in a desktop window.

use tessera_ui::{
    EntryPoint, remember,
    renderer::{TesseraConfig, WindowConfig},
    tessera,
};
use tessera_webview::{
    IpcPolicy, WebViewController, WebViewEvent, WebViewOptions, WebViewPackage, WebViewSource,
    webview,
};

const PAGE: &str = r#"<!doctype html><title>Tessera native browser</title>
<style>body{margin:0;background:#19b46a;color:#081f16;font:22px sans-serif;min-height:240vh}
 main{padding:32px} button,input{box-sizing:border-box;font:inherit;height:36px;margin-right:16px}
 button{width:120px} input{width:220px}</style>
<main><button id="button">Activate</button><input aria-label="Editor" placeholder="Enter text"></main>
<script>
 const post = value => window.ipc.postMessage(value);
 document.querySelector('button').onclick = () => post('activated');
  const editor = document.querySelector('input');
  editor.onfocus = () => post('editor-focused');
  editor.oninput = event => post('text:' + event.target.value);
  editor.addEventListener('compositionupdate', event => post('preedit:' + event.data));
  editor.addEventListener('compositionend', event => post('composition-end:' + event.data));
 window.onscroll = () => post('scroll:' + window.scrollY);
 post('page-ready');
</script>"#;

#[tessera]
fn app() {
    let controller = remember(WebViewController::new).get();
    let focus_editor = std::env::args().any(|arg| arg == "--focus-editor" || arg == "--ime-probe");
    webview(WebViewSource::html(PAGE))
        .controller(controller)
        .options(WebViewOptions {
            ipc: IpcPolicy::AllowAll,
            ..Default::default()
        })
        .on_event(move |event| match event {
            WebViewEvent::Message { body, .. } if focus_editor && body == "page-ready" => {
                controller.focus().unwrap();
                controller
                    .evaluate_script("document.querySelector('input').focus()")
                    .unwrap();
                println!("page-ready");
            }
            WebViewEvent::Message { body, .. } => println!("{body}"),
            event => println!("{event:?}"),
        });
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = TesseraConfig {
        window: WindowConfig {
            transparent: false,
            ..Default::default()
        },
        ..Default::default()
    };
    EntryPoint::new(app)
        .package(WebViewPackage)
        .config(config)
        .run_desktop()?;
    Ok(())
}
