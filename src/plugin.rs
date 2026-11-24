use std::collections::HashMap;

use async_channel::{Receiver, Sender};
use futures_channel::oneshot;
use serde::Deserialize;
use tessera_ui::{
    CallbackWith, FloatRect, Plugin, PluginContext, PluginResult, Task, spawn_local, with_plugin,
};

use crate::{WebViewError, WebViewEvent, WebViewOptions, WebViewSource, backend, next_id};

#[derive(Deserialize)]
pub(crate) struct Envelope {
    pub id: u64,
    pub generation: u64,
    pub payload: Payload,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Payload {
    Event { event: WebViewEvent },
    Script { request: u64, value: String },
}

#[derive(Clone)]
pub(crate) struct EventSink {
    pub id: u64,
    pub generation: u64,
    pub sender: Sender<Envelope>,
}

impl EventSink {
    pub fn event(&self, event: WebViewEvent) {
        let _ = self.sender.try_send(Envelope {
            id: self.id,
            generation: self.generation,
            payload: Payload::Event { event },
        });
    }

    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "ios"
    ))]
    pub fn script(&self, request: u64, value: String) {
        let _ = self.sender.try_send(Envelope {
            id: self.id,
            generation: self.generation,
            payload: Payload::Script { request, value },
        });
    }
}

fn valid_bounds(bounds: FloatRect) -> bool {
    [bounds.x, bounds.y, bounds.width, bounds.height]
        .iter()
        .all(|value| value.is_finite())
        && bounds.width > 0.0
        && bounds.height > 0.0
}

struct Entry {
    owner: u64,
    generation: u64,
    declared_source: WebViewSource,
    source: WebViewSource,
    options: WebViewOptions,
    on_event: Option<CallbackWith<WebViewEvent>>,
    bounds: Option<FloatRect>,
    native: Option<backend::NativeView>,
    ready: bool,
    last_error: Option<WebViewError>,
    evaluations: HashMap<u64, oneshot::Sender<String>>,
}

impl Entry {
    fn release(&mut self) {
        self.generation = next_id();
        self.native = None;
        self.ready = false;
        self.evaluations.clear();
    }
}

/// Owns native browsers, dispatches callbacks on the UI session, and handles resume.
pub struct WebViewPlugin {
    context: Option<PluginContext>,
    entries: HashMap<u64, Entry>,
    sender: Sender<Envelope>,
    receiver: Receiver<Envelope>,
    events: Option<Task>,
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pump: Option<Task>,
}

impl Default for WebViewPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl WebViewPlugin {
    /// Creates a plugin. Register it once with the application's entry point.
    pub fn new() -> Self {
        let (sender, receiver) = async_channel::bounded(256);
        Self {
            context: None,
            entries: HashMap::new(),
            sender,
            receiver,
            events: None,
            #[cfg(any(target_os = "android", target_os = "linux"))]
            pump: None,
        }
    }

    pub(crate) fn configure(
        &mut self,
        id: u64,
        owner: u64,
        source: WebViewSource,
        options: WebViewOptions,
        on_event: Option<CallbackWith<WebViewEvent>>,
    ) -> Result<(), WebViewError> {
        if let Some(entry) = self.entries.get(&id)
            && entry.owner != owner
        {
            return Err(WebViewError::ControllerInUse);
        }
        let entry = self.entries.entry(id).or_insert_with(|| Entry {
            owner,
            generation: next_id(),
            declared_source: source.clone(),
            source: source.clone(),
            options: options.clone(),
            on_event,
            bounds: None,
            native: None,
            ready: false,
            last_error: None,
            evaluations: HashMap::new(),
        });
        entry.on_event = on_event;
        let changed = entry.options != options || entry.declared_source != source;
        if entry.options != options {
            entry.release();
            entry.options = options;
        }
        if entry.declared_source != source {
            entry.evaluations.clear();
            entry.declared_source = source.clone();
            entry.source = source;
            if let Some(view) = &entry.native
                && let Err(error) = entry
                    .source
                    .validate(&entry.options.navigation)
                    .and_then(|_| view.load(&entry.source))
            {
                self.report(id, error);
                return Ok(());
            }
        }
        if changed {
            entry.last_error = None;
        }
        self.reconcile_and_report(id);
        Ok(())
    }

    pub(crate) fn position(
        &mut self,
        id: u64,
        owner: u64,
        bounds: FloatRect,
    ) -> Result<(), WebViewError> {
        self.owned(id, owner)?.bounds = Some(bounds);
        Ok(())
    }

    fn owned(&mut self, id: u64, owner: u64) -> Result<&mut Entry, WebViewError> {
        let entry = self.entries.get_mut(&id).ok_or(WebViewError::NotMounted)?;
        if entry.owner != owner {
            return Err(WebViewError::ControllerInUse);
        }
        Ok(entry)
    }

    #[cfg(any(
        target_os = "android",
        target_os = "ios",
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ))]
    pub(crate) fn factory(
        &mut self,
        id: u64,
        owner: u64,
    ) -> Result<Option<(u64, backend::Factory)>, WebViewError> {
        let entry = self.owned(id, owner)?;
        Ok(entry
            .native
            .as_ref()
            .map(|view| (entry.generation, view.factory())))
    }

    fn sink(&self, id: u64) -> EventSink {
        EventSink {
            id,
            generation: self.entries[&id].generation,
            sender: self.sender.clone(),
        }
    }

    fn report(&mut self, id: u64, error: WebViewError) {
        let entry = self.entries.get_mut(&id).unwrap();
        if entry.last_error.as_ref() != Some(&error) {
            entry.last_error = Some(error.clone());
            self.sink(id).event(WebViewEvent::Error { error });
        }
    }

    fn reconcile_and_report(&mut self, id: u64) {
        if let Err(error) = self.reconcile(id) {
            self.report(id, error);
        }
    }

    fn reconcile(&mut self, id: u64) -> Result<(), WebViewError> {
        let sink = self.sink(id);
        let entry = self.entries.get_mut(&id).unwrap();
        entry.options.validate()?;
        entry.source.validate(&entry.options.navigation)?;
        if let Some(context) = &self.context
            && entry.native.is_none()
        {
            entry.native = Some(backend::NativeView::new(
                context,
                &entry.source,
                &entry.options,
                sink,
            )?);
        }
        Ok(())
    }

    pub(crate) fn native(&self, id: u64) -> Result<&backend::NativeView, WebViewError> {
        let entry = self.entries.get(&id).ok_or(WebViewError::NotMounted)?;
        self.context.as_ref().ok_or(WebViewError::HostNotResumed)?;
        if !entry.ready {
            return Err(WebViewError::NotReady);
        }
        entry.native.as_ref().ok_or(WebViewError::NotReady)
    }

    pub(crate) fn bounds(&self, id: u64) -> Result<FloatRect, WebViewError> {
        self.native(id)?;
        self.entries[&id]
            .bounds
            .filter(|bounds| valid_bounds(*bounds))
            .ok_or(WebViewError::NotReady)
    }

    pub(crate) fn load(&mut self, id: u64, source: WebViewSource) -> Result<(), WebViewError> {
        let entry = self.entries.get(&id).ok_or(WebViewError::NotMounted)?;
        source.validate(&entry.options.navigation)?;
        self.native(id)?.load(&source)?;
        let entry = self.entries.get_mut(&id).unwrap();
        entry.evaluations.clear();
        entry.source = source;
        Ok(())
    }

    pub(crate) fn evaluate(
        &mut self,
        id: u64,
        script: String,
    ) -> Result<oneshot::Receiver<String>, WebViewError> {
        self.native(id)?;
        let request = next_id();
        let (sender, receiver) = oneshot::channel();
        let entry = self.entries.get_mut(&id).unwrap();
        // Bound abandoned requests as well as native event traffic.
        entry.evaluations.retain(|_, sender| !sender.is_canceled());
        if entry.evaluations.len() >= 128 {
            return Err(WebViewError::Native("too many pending evaluations".into()));
        }
        entry.evaluations.insert(request, sender);
        if let Err(error) = entry.native.as_ref().unwrap().evaluate(&script, request) {
            entry.evaluations.remove(&request);
            return Err(error);
        }
        Ok(receiver)
    }

    pub(crate) fn detach(&mut self, id: u64, owner: u64) {
        if self
            .entries
            .get(&id)
            .is_some_and(|entry| entry.owner == owner)
        {
            self.entries.remove(&id);
        }
    }

    fn dispatch(
        &mut self,
        envelope: Envelope,
    ) -> Option<(CallbackWith<WebViewEvent>, WebViewEvent)> {
        let entry = self.entries.get_mut(&envelope.id)?;
        if entry.generation != envelope.generation {
            return None;
        }
        match envelope.payload {
            Payload::Event { event } => {
                if matches!(event, WebViewEvent::Ready) {
                    entry.ready = true;
                }
                entry.on_event.map(|callback| (callback, event))
            }
            Payload::Script { request, value } => {
                if let Some(sender) = entry.evaluations.remove(&request) {
                    let _ = sender.send(value);
                }
                None
            }
        }
    }
}

impl Plugin for WebViewPlugin {
    fn on_resumed(&mut self, context: &PluginContext) -> PluginResult {
        #[cfg(not(any(
            target_os = "android",
            target_os = "ios",
            target_os = "windows",
            target_os = "macos",
            target_os = "linux"
        )))]
        backend::initialize(context)?;
        if self.events.is_none() {
            let receiver = self.receiver.clone();
            self.events = Some(spawn_local(async move {
                while let Ok(envelope) = receiver.recv().await {
                    let notification =
                        with_plugin::<WebViewPlugin, _>(|plugin| plugin.dispatch(envelope))
                            .flatten();
                    if let Some((callback, event)) = notification {
                        callback.call(event);
                    }
                }
            }));
        }
        #[cfg(target_os = "linux")]
        {
            self.pump = Some(spawn_local(async move {
                loop {
                    backend::pump();
                    tokio::time::sleep(std::time::Duration::from_millis(16)).await;
                }
            }));
        }
        #[cfg(target_os = "android")]
        {
            let sender = self.sender.clone();
            self.pump = Some(spawn_local(async move {
                loop {
                    let _ = with_plugin::<WebViewPlugin, _>(|plugin| {
                        for entry in plugin.entries.values() {
                            if let Some(view) = &entry.native {
                                view.pump(&sender);
                            }
                        }
                    });
                    tokio::time::sleep(std::time::Duration::from_millis(16)).await;
                }
            }));
        }
        self.context = Some(context.clone());
        for id in self.entries.keys().copied().collect::<Vec<_>>() {
            self.reconcile_and_report(id);
        }
        Ok(())
    }

    fn on_suspended(&mut self, _context: &PluginContext) -> PluginResult {
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            self.pump = None;
        }
        for entry in self.entries.values_mut() {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            entry.release();
            #[cfg(any(target_os = "android", target_os = "ios"))]
            entry.evaluations.clear();
        }
        self.context = None;
        Ok(())
    }

    fn on_shutdown(&mut self, context: &PluginContext) -> PluginResult {
        self.on_suspended(context)?;
        self.entries.clear();
        self.events = None;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
