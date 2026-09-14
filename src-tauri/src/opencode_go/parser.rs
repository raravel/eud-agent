use super::*;

mod anthropic;
mod chat;
mod responses;
use anthropic::*;
use chat::*;
use responses::*;

pub(super) struct ResponsesToolCall {
    id: String,
    item_id: Option<String>,
    name: String,
    arguments: String,
}

pub(super) enum WireParser {
    Responses {
        step: NormalizedAssistantStep,
        calls: Vec<ResponsesToolCall>,
    },
    Chat {
        step: NormalizedAssistantStep,
        calls: BTreeMap<u64, (String, String, String)>,
    },
    Anthropic {
        step: NormalizedAssistantStep,
        calls: BTreeMap<u64, (String, String, String)>,
    },
}

impl WireParser {
    pub(super) fn new(wire: OpenCodeGoWire) -> Self {
        match wire {
            OpenCodeGoWire::Responses => Self::Responses {
                step: NormalizedAssistantStep::default(),
                calls: Vec::new(),
            },
            OpenCodeGoWire::ChatCompletions => Self::Chat {
                step: NormalizedAssistantStep::default(),
                calls: BTreeMap::new(),
            },
            OpenCodeGoWire::AnthropicMessages => Self::Anthropic {
                step: NormalizedAssistantStep::default(),
                calls: BTreeMap::new(),
            },
        }
    }

    pub(super) fn apply(&mut self, event: Option<&str>, value: Value) -> Result<(), String> {
        match self {
            Self::Responses { step, calls } => parse_responses_event(step, calls, event, &value),
            Self::Chat { step, calls } => parse_chat_event(step, calls, &value),
            Self::Anthropic { step, calls } => parse_anthropic_event(step, calls, event, &value),
        }
    }

    pub(super) fn finish(self) -> Result<NormalizedAssistantStep, String> {
        let (mut step, calls) = match self {
            Self::Responses { step, calls } => {
                let calls = calls
                    .into_iter()
                    .map(|call| (call.id, call.name, call.arguments))
                    .collect::<Vec<_>>();
                (step, calls)
            }
            Self::Chat { step, calls } | Self::Anthropic { step, calls } => {
                (step, calls.into_values().collect::<Vec<_>>())
            }
        };
        for (id, name, arguments) in calls {
            let arguments = serde_json::from_str(if arguments.trim().is_empty() {
                "{}"
            } else {
                &arguments
            })
            .map_err(|_| "provider returned invalid tool arguments".to_string())?;
            step.tool_calls.push(DirectToolCall {
                id,
                name,
                arguments,
            });
        }
        if step.text.is_empty() && step.tool_calls.is_empty() {
            return Err("provider returned an empty response".to_string());
        }
        Ok(step)
    }
}
