use async_channel::Sender;
use serde_json::json;
use tessera_ui::{
    PluginContext,
    android::{
        ActivityRef, activity,
        jni::{
            AndroidJniError, JavaClass, JavaObject, JniReturn,
            internal::{JNIEnv, JValueOwned},
        },
        platform_views::AndroidPlatformViewFactoryHandle,
    },
    winit::platform::android::activity::AndroidApp,
};

use crate::{
    IpcPolicy, WebViewError, WebViewOptions, WebViewSource,
    plugin::{Envelope, EventSink},
};

use super::{Command, error};

#[derive(Clone)]
struct BrowserFactoryClass;

impl JavaClass for BrowserFactoryClass {
    fn name() -> &'static str {
        "com.tessera.webview.WebViewPlugin$Factory"
    }
}
type BrowserFactory = JavaObject<BrowserFactoryClass>;
pub(crate) type Factory = AndroidPlatformViewFactoryHandle;

struct FactoryProxy(Factory);

impl JniReturn for FactoryProxy {
    fn signature() -> std::borrow::Cow<'static, str> {
        "Ljava/lang/Object;".into()
    }

    fn from_jvalue<'a>(
        env: &mut JNIEnv<'a>,
        value: JValueOwned<'a>,
    ) -> Result<Self, AndroidJniError> {
        Ok(Self(Factory::new(env.new_global_ref(value.l()?)?)))
    }
}

tessera_ui::android::jni_bind! {
    class "com.tessera.webview.WebViewPlugin" as Bridge {
        // This Tessera revision's jni_bind! requires at least one argument.
        fn supportsPlatformViews(activity: ActivityRef) -> bool;
        fn newFactory(activity: ActivityRef, descriptor: &str) -> BrowserFactory;
        fn platformFactory(factory: BrowserFactory) -> FactoryProxy;
        fn command(factory: BrowserFactory, command: &str) -> ();
        fn query(factory: BrowserFactory) -> String;
        fn poll(factory: BrowserFactory) -> String;
    }
}

pub(crate) struct NativeView {
    app: AndroidApp,
    holder: BrowserFactory,
    factory: Factory,
}

impl NativeView {
    pub fn new(
        context: &PluginContext,
        source: &WebViewSource,
        options: &WebViewOptions,
        sink: EventSink,
    ) -> Result<Self, WebViewError> {
        let app = context.android_app();
        if !Bridge::supportsPlatformViews(app, activity(app)).map_err(error)? {
            return Err(WebViewError::UnsupportedOperation(
                "embedded WebView requires Android API 30 or newer".into(),
            ));
        }
        if options.incognito || options.devtools {
            return Err(WebViewError::UnsupportedOperation(
                "Android does not provide per-view incognito or developer-tools isolation".into(),
            ));
        }
        let mut options = options.clone();
        if let IpcPolicy::Origins(origins) = &mut options.ipc {
            for origin in origins {
                *origin = url::Url::parse(origin)
                    .map_err(error)?
                    .origin()
                    .ascii_serialization();
            }
        }
        let descriptor = json!({ "id": sink.id.to_string(), "generation": sink.generation,
            "source": source, "options": options })
        .to_string();
        let holder = Bridge::newFactory(app, activity(app), &descriptor).map_err(error)?;
        let factory = Bridge::platformFactory(app, holder.clone())
            .map_err(error)?
            .0;
        Ok(Self {
            app: app.clone(),
            holder,
            factory,
        })
    }

    pub fn factory(&self) -> Factory {
        self.factory.clone()
    }

    pub fn pump(&self, sender: &Sender<Envelope>) {
        if let Ok(events) = Bridge::poll(&self.app, self.holder.clone())
            && let Ok(events) = serde_json::from_str::<Vec<Envelope>>(&events)
        {
            for event in events {
                let _ = sender.try_send(event);
            }
        }
    }

    fn send(&self, value: serde_json::Value) -> Result<(), WebViewError> {
        Bridge::command(&self.app, self.holder.clone(), &value.to_string()).map_err(error)
    }
    pub fn load(&self, source: &WebViewSource) -> Result<(), WebViewError> {
        self.send(json!({"kind":"load", "value":source}))
    }
    pub fn evaluate(&self, script: &str, request: u64) -> Result<(), WebViewError> {
        self.send(json!({"kind":"evaluate", "value":script, "request":request}))
    }
    fn query(&self) -> Result<serde_json::Value, WebViewError> {
        serde_json::from_str(&Bridge::query(&self.app, self.holder.clone()).map_err(error)?)
            .map_err(error)
    }
    pub fn url(&self) -> Result<String, WebViewError> {
        Ok(self.query()?["url"]
            .as_str()
            .unwrap_or("about:blank")
            .into())
    }
    pub fn can_go_back(&self) -> Result<bool, WebViewError> {
        Ok(self.query()?["back"].as_bool().unwrap_or(false))
    }
    pub fn can_go_forward(&self) -> Result<bool, WebViewError> {
        Ok(self.query()?["forward"].as_bool().unwrap_or(false))
    }
    pub fn command(&self, command: Command) -> Result<(), WebViewError> {
        self.send(match command {
            Command::Script(script) => json!({"kind":"script", "value":script}),
            Command::Back => json!({"kind":"back"}),
            Command::Forward => json!({"kind":"forward"}),
            Command::Reload => json!({"kind":"reload"}),
            Command::Focus => json!({"kind":"focus"}),
            Command::Zoom(value) => json!({"kind":"zoom", "value":value}),
            Command::ClearData => json!({"kind":"clear_data"}),
        })
    }
}
