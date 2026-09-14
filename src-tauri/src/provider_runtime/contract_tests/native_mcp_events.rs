use parking_lot::Mutex;
use serde_json::Value;

use crate::provider_runtime::{
    AdapterEventKind, NormalizedBlock, ProviderRuntimeError, RuntimeEventSink,
};

#[derive(Default)]
pub(super) struct NativeEvents {
    pub(super) observations: Mutex<Vec<(String, Option<Value>)>>,
    pub(super) authoritative_tools: Mutex<Vec<AuthoritativeToolEvent>>,
    pub(super) text: Mutex<String>,
    pub(super) finishes: Mutex<Vec<bool>>,
}

#[derive(Debug, PartialEq)]
pub(super) enum AuthoritativeToolEvent {
    Call {
        id: String,
        name: String,
        arguments: Value,
    },
    Result {
        id: String,
        name: String,
        result: Value,
        is_error: bool,
    },
}

impl RuntimeEventSink for NativeEvents {
    fn emit(&self, event: &AdapterEventKind) -> Result<(), ProviderRuntimeError> {
        match event {
            AdapterEventKind::NativeToolObservation { name, result, .. } => {
                self.observations
                    .lock()
                    .push((name.clone(), result.clone()));
            }
            AdapterEventKind::Block(NormalizedBlock::Text { text, .. }) => {
                self.text.lock().push_str(text);
            }
            AdapterEventKind::Block(NormalizedBlock::ToolCall { call, .. }) => {
                self.authoritative_tools
                    .lock()
                    .push(AuthoritativeToolEvent::Call {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    });
            }
            AdapterEventKind::Block(NormalizedBlock::ToolResult { result, .. }) => {
                self.authoritative_tools
                    .lock()
                    .push(AuthoritativeToolEvent::Result {
                        id: result.id.clone(),
                        name: result.name.clone(),
                        result: result.result.clone(),
                        is_error: result.is_error,
                    });
            }
            AdapterEventKind::ResponseFinished { complete, .. } => {
                self.finishes.lock().push(*complete);
            }
            _ => {}
        }
        Ok(())
    }
}
