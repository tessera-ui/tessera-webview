use tessera_ui::PluginContext;

use crate::{WebViewError, WebViewOptions, WebViewSource, plugin::EventSink};

use super::Command;

pub(crate) fn initialize(_: &PluginContext) -> Result<(), WebViewError> {
    Err(WebViewError::UnsupportedPlatform)
}

pub(crate) struct NativeView;

impl NativeView {
    pub fn new(
        _: &PluginContext,
        _: &WebViewSource,
        _: &WebViewOptions,
        _: EventSink,
    ) -> Result<Self, WebViewError> {
        Err(WebViewError::UnsupportedPlatform)
    }
    pub fn load(&self, _: &WebViewSource) -> Result<(), WebViewError> {
        Err(WebViewError::UnsupportedPlatform)
    }
    pub fn url(&self) -> Result<String, WebViewError> {
        Err(WebViewError::UnsupportedPlatform)
    }
    pub fn can_go_back(&self) -> Result<bool, WebViewError> {
        Err(WebViewError::UnsupportedPlatform)
    }
    pub fn can_go_forward(&self) -> Result<bool, WebViewError> {
        Err(WebViewError::UnsupportedPlatform)
    }
    pub fn evaluate(&self, _: &str, _: u64) -> Result<(), WebViewError> {
        Err(WebViewError::UnsupportedPlatform)
    }
    pub fn command(&self, _: Command) -> Result<(), WebViewError> {
        Err(WebViewError::UnsupportedPlatform)
    }
}
