// tessera-macros 1.0 generated setters trigger this lint on Rust 1.98.
#![allow(clippy::mem_replace_option_with_some, clippy::needless_update)]

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use async_channel::Receiver;
use tessera_foundation::modifier::ModifierExt as _;
use tessera_ui::{
    Dp, EntryPoint, Modifier, Plugin, PluginContext, Stack, State, launch_once, layout::layout,
    remember, tessera, with_plugin,
};
use tessera_webview::{
    IpcPolicy, NavigationPolicy, WebViewController, WebViewEvent, WebViewOptions, WebViewPackage,
    WebViewPlugin, WebViewSource, webview,
};

static SUCCESS: AtomicBool = AtomicBool::new(false);

#[tessera]
fn app(origin: String) {
    let controller = remember(WebViewController::new).get();
    let secondary = remember(WebViewController::new).get();
    let source = remember(|| WebViewSource::url(format!("{origin}/one")));
    let width = remember(|| 320.0_f32);
    let mounted = remember(|| true);
    let agent = remember(|| Option::<String>::None);
    let messages = remember(async_channel::unbounded::<WebViewEvent>).get();
    let page_origin = origin.clone();
    let sender = messages.0.clone();
    layout(Stack).child(move || {
        if mounted.get() {
            let sender = sender.clone();
            webview(source.get())
                .controller(controller)
                .options(WebViewOptions {
                    navigation: NavigationPolicy::Origins(vec![page_origin.clone()]),
                    ipc: IpcPolicy::Origins(vec![page_origin.clone()]),
                    initialization_scripts: vec!["window.__initialized = 7".into()],
                    user_agent: agent.get(),
                    ..Default::default()
                })
                .modifier(
                    Modifier::new()
                        .offset(Dp(20.0), Dp(20.0))
                        .required_size(Dp(f64::from(width.get())), Dp(220.0)),
                )
                .on_event(move |event: WebViewEvent| {
                    println!("{event:?}");
                    sender.try_send(event).unwrap();
                });
        }
        webview(WebViewSource::html(
            "<title>independent</title><p>Second native browser</p>",
        ))
        .controller(secondary)
        .modifier(
            Modifier::new()
                .offset(Dp(20.0), Dp(270.0))
                .required_size(Dp(320.0), Dp(100.0)),
        );
    });
    launch_once(async move {
        let outcome = tokio::time::timeout(Duration::from_secs(30), async move {
            exercise(
                controller, &origin, source, width, mounted, agent, messages.1,
            )
            .await?;
            // The second browser survives the first browser's navigation,
            // option recreation, suspension, and unmount without sharing IPC.
            until(|| secondary.is_ready()).await;
            while secondary.evaluate("document.title").await.unwrap() != "\"independent\"" {
                tokio::time::sleep(Duration::from_millis(16)).await;
            }
            assert_eq!(
                secondary.evaluate("typeof window.ipc").await.unwrap(),
                "\"undefined\""
            );
            Ok::<(), String>(())
        })
        .await;
        match outcome {
            Ok(Ok(())) => {
                SUCCESS.store(true, Ordering::SeqCst);
                println!(
                    "SMOKE PASS: load, title, IPC, evaluation, navigation, history, resize, source update, options, suspend/resume, unmount, independent browsers"
                );
            }
            Ok(Err(error)) => eprintln!("SMOKE FAIL: {error}"),
            Err(_) => eprintln!("SMOKE FAIL: timed out"),
        }
        if let Some(context) = PluginContext::current() {
            context.desktop().request_close();
        }
    });
}

async fn wait(
    events: &Receiver<WebViewEvent>,
    predicate: impl Fn(&WebViewEvent) -> bool,
) -> Result<(), String> {
    loop {
        let event = events.recv().await.map_err(|error| error.to_string())?;
        if let WebViewEvent::Error { error } = &event {
            return Err(error.to_string());
        }
        if predicate(&event) {
            return Ok(());
        }
    }
}

async fn loaded(events: &Receiver<WebViewEvent>, suffix: &str) -> Result<(), String> {
    wait(
        events,
        |event| matches!(event, WebViewEvent::LoadFinished { url } if url.ends_with(suffix)),
    )
    .await
}

async fn until(mut predicate: impl FnMut() -> bool) {
    while !predicate() {
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}

async fn exercise(
    controller: WebViewController,
    origin: &str,
    source: State<WebViewSource>,
    width: State<f32>,
    mounted: State<bool>,
    agent: State<Option<String>>,
    events: Receiver<WebViewEvent>,
) -> Result<(), String> {
    // These are real browser/OS checks; the outer deadline bounds all waits.
    loaded(&events, "/one").await?;
    until(|| controller.is_ready()).await;
    assert_eq!(controller.evaluate("6 * 7").await.unwrap(), "42");
    assert_eq!(
        controller.evaluate("window.__initialized").await.unwrap(),
        "7"
    );
    assert_eq!(
        controller.evaluate("document.title").await.unwrap(),
        "\"one\""
    );
    controller
        .evaluate_script("window.ipc.postMessage('from-page')")
        .unwrap();
    wait(
        &events,
        |event| matches!(event, WebViewEvent::Message { body, .. } if body == "from-page"),
    )
    .await?;
    controller.load_url(format!("{origin}/two")).unwrap();
    loaded(&events, "/two").await?;
    assert!(controller.can_go_back().unwrap());
    controller.go_back().unwrap();
    loaded(&events, "/one").await?;
    assert!(controller.can_go_forward().unwrap());
    controller.go_forward().unwrap();
    loaded(&events, "/two").await?;
    controller.reload().unwrap();
    loaded(&events, "/two").await?;
    controller.evaluate_script("window.__marker = 123").unwrap();
    width.set(360.0);
    let scale = PluginContext::current().unwrap().window().scale_factor() as f32;
    until(|| {
        controller
            .bounds()
            .is_ok_and(|bounds| (bounds.width - 360.0 * scale).abs() < 1.5)
    })
    .await;
    assert!(controller.url().unwrap().ends_with("/two"));
    assert_eq!(controller.evaluate("window.__marker").await.unwrap(), "123");
    controller
        .evaluate_script("location.href = 'https://blocked.invalid/'")
        .unwrap();
    wait(&events, |event| {
        matches!(event, WebViewEvent::NavigationBlocked { .. })
    })
    .await?;
    assert!(controller.url().unwrap().ends_with("/two"));
    source.set(WebViewSource::html(
        "<title>declarative</title><h1>Updated without relayout</h1>",
    ));
    loaded(&events, "about:blank").await?;
    assert_eq!(
        controller.evaluate("document.title").await.unwrap(),
        "\"declarative\""
    );
    agent.set(Some("TesseraSmoke/1.0".into()));
    wait(&events, |event| matches!(event, WebViewEvent::Ready)).await?;
    loaded(&events, "about:blank").await?;
    assert_eq!(
        controller.evaluate("navigator.userAgent").await.unwrap(),
        "\"TesseraSmoke/1.0\""
    );
    let context = PluginContext::current().unwrap();
    with_plugin::<WebViewPlugin, _>(|plugin| plugin.on_suspended(&context))
        .unwrap()
        .unwrap();
    assert!(!controller.is_ready());
    with_plugin::<WebViewPlugin, _>(|plugin| plugin.on_resumed(&context))
        .unwrap()
        .unwrap();
    wait(&events, |event| matches!(event, WebViewEvent::Ready)).await?;
    loaded(&events, "about:blank").await?;
    assert_eq!(
        controller.evaluate("document.title").await.unwrap(),
        "\"declarative\""
    );
    mounted.set(false);
    until(|| !controller.is_ready()).await;
    assert!(matches!(
        controller.reload(),
        Err(tessera_webview::WebViewError::NotMounted)
    ));
    Ok(())
}

fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = [0; 4096];
            let Ok(length) = stream.read(&mut request) else {
                continue;
            };
            let path = String::from_utf8_lossy(&request[..length])
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .trim_start_matches('/')
                .to_owned();
            let body =
                format!("<!doctype html><meta charset=utf-8><title>{path}</title><h1>{path}</h1>");
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    format!("http://{address}")
}

#[tessera_ui::entry]
fn entry() -> EntryPoint {
    let origin = serve();
    EntryPoint::new(move || app(origin.clone())).package(WebViewPackage)
}

fn main() {
    entry().run_desktop().unwrap();
    assert!(
        SUCCESS.load(Ordering::SeqCst),
        "native WebView smoke test did not pass"
    );
}
