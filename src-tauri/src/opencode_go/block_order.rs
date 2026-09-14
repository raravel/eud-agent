use super::*;
use std::collections::HashSet;

enum PendingBlock {
    Block(NormalizedBlock),
    Tool(String),
}

#[derive(Default)]
pub(super) struct BlockOrder {
    tail: Vec<PendingBlock>,
    tool_ids: HashSet<String>,
}

impl BlockOrder {
    pub(super) fn observe(&mut self, wire: OpenCodeGoWire, event: Option<&str>, value: &Value) {
        let kind = event.or_else(|| value.get("type").and_then(Value::as_str));
        let id = match (wire, kind) {
            (
                OpenCodeGoWire::Responses,
                Some("response.output_item.added" | "response.output_item.done"),
            ) => value
                .get("item")
                .filter(|item| item["type"] == "function_call")
                .and_then(|item| item.get("call_id").or_else(|| item.get("id")))
                .and_then(Value::as_str),
            (OpenCodeGoWire::AnthropicMessages, Some("content_block_start")) => value
                .get("content_block")
                .filter(|item| item["type"] == "tool_use")
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str),
            _ => None,
        };
        if let Some(id) = id {
            if self.tool_ids.insert(id.to_string()) {
                self.tail.push(PendingBlock::Tool(id.to_string()));
            }
        }
    }

    pub(super) fn push(&mut self, block: NormalizedBlock) -> Option<NormalizedBlock> {
        if self.tool_ids.is_empty() {
            return Some(block);
        }
        // Keep the bounded tail behind unfinished calls until the entire response validates.
        self.tail.push(PendingBlock::Block(block));
        None
    }

    pub(super) fn finish(
        self,
        step: &NormalizedAssistantStep,
        continuation: Option<&ProviderContinuation>,
        response_id: &str,
    ) -> Result<Vec<NormalizedBlock>, String> {
        let mut calls = step
            .tool_calls
            .iter()
            .map(|call| (call.id.clone(), call))
            .collect::<HashMap<_, _>>();
        if calls.len() != step.tool_calls.len() {
            return Err("provider returned duplicate tool call id".into());
        }
        let tail = if self.tool_ids.is_empty() {
            // Chat has separate content and indexed tool-call fields, not ordered content items.
            step.tool_calls
                .iter()
                .map(|call| PendingBlock::Tool(call.id.clone()))
                .collect()
        } else {
            self.tail
        };
        let mut blocks = Vec::with_capacity(tail.len());
        for pending in tail {
            blocks.push(match pending {
                PendingBlock::Block(block) => block,
                PendingBlock::Tool(id) => NormalizedBlock::ToolCall {
                    response_id: response_id.to_string(),
                    batch_id: format!("{response_id}-tools"),
                    call: calls
                        .remove(&id)
                        .ok_or_else(|| "provider tool item did not complete".to_string())?
                        .clone(),
                    continuation: continuation.cloned(),
                },
            });
        }
        if !calls.is_empty() {
            return Err("provider tool call has no ordered item".into());
        }
        Ok(blocks)
    }
}
