use async_channel::Sender;
use serde_json::json;
use tessera_ui::{
    FloatRect, PluginContext,
    android::{ActivityRef, activity},
    winit::platform::android::activity::AndroidApp,
};

use crate::{
    IpcPolicy, WebViewError, WebViewOptions, WebViewSource,
    plugin::{Envelope, EventSink},
};

use super::{Command, error};

tessera_ui::android::jni_bind! {
    class "com.tessera.webview.WebViewPlugin" as Bridge {
        fn create(activity: ActivityRef, descriptor: &str) -> ();
        fn command(activity: ActivityRef, id: &str, command: &str) -> ();
        fn query(id: &str) -> String;
        fn poll(activity: ActivityRef) -> String;
    }
}

pub(crate) fn initialize(context: &PluginContext) -> Result<(), WebViewError> {
    Bridge::poll(context.android_app(), activity(context.android_app()))
        .map(|_| ())
        .map_err(error)
}

pub(crate) fn pump(context: &PluginContext, sender: &Sender<Envelope>) {
    if let Ok(events) = Bridge::poll(context.android_app(), activity(context.android_app()))
        && let Ok(events) = serde_json::from_str::<Vec<Envelope>>(&events)
    {
        for event in events {
            let _ = sender.try_send(event);
        }
    }
}

pub(crate) struct NativeView {
    app: AndroidApp,
    id: String,
    generation: u64,
}

impl NativeView {
    pub fn new(
        context: &PluginContext,
        source: &WebViewSource,
        options: &WebViewOptions,
        bounds: FloatRect,
        sink: EventSink,
    ) -> Result<Self, WebViewError> {
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
            "source": source, "options": options, "bounds": [bounds.x, bounds.y, bounds.width, bounds.height] }).to_string();
        Bridge::create(
            context.android_app(),
            activity(context.android_app()),
            &descriptor,
        )
        .map_err(error)?;
        Ok(Self {
            app: context.android_app().clone(),
            id: sink.id.to_string(),
            generation: sink.generation,
        })
    }

    fn send(&self, mut value: serde_json::Value) -> Result<(), WebViewError> {
        value["generation"] = self.generation.into();
        Bridge::command(&self.app, activity(&self.app), &self.id, &value.to_string()).map_err(error)
    }

    pub fn set_bounds(&self, bounds: FloatRect, _scale: f64) -> Result<(), WebViewError> {
        self.send(json!({"kind":"bounds", "value":[bounds.x,bounds.y,bounds.width,bounds.height]}))
    }
    pub fn set_visible(&self, visible: bool) -> Result<(), WebViewError> {
        self.send(json!({"kind":"visible", "value":visible}))
    }
    pub fn load(&self, source: &WebViewSource) -> Result<(), WebViewError> {
        self.send(json!({"kind":"load", "value":source}))
    }
    pub fn evaluate(&self, script: &str, request: u64) -> Result<(), WebViewError> {
        self.send(json!({"kind":"evaluate", "value":script, "request":request}))
    }
    fn query(&self) -> Result<serde_json::Value, WebViewError> {
        serde_json::from_str(&Bridge::query(&self.app, &self.id).map_err(error)?).map_err(error)
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
    pub fn bounds(&self, _scale: f64) -> Result<FloatRect, WebViewError> {
        let state = self.query()?;
        let bounds = state["bounds"].as_array().ok_or(WebViewError::NotReady)?;
        Ok(FloatRect {
            x: bounds[0].as_f64().unwrap_or(0.0) as f32,
            y: bounds[1].as_f64().unwrap_or(0.0) as f32,
            width: bounds[2].as_f64().unwrap_or(0.0) as f32,
            height: bounds[3].as_f64().unwrap_or(0.0) as f32,
        })
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

impl Drop for NativeView {
    fn drop(&mut self) {
        let _ = self.send(json!({"kind":"destroy"}));
    }
}
