use crate::{opencode_go::OpenCodeGoWire, provider::ProviderModel};

pub(super) struct LiveRow {
    pub model: ProviderModel,
    pub wire: OpenCodeGoWire,
}

const WIRES: [OpenCodeGoWire; 3] = [
    OpenCodeGoWire::Responses,
    OpenCodeGoWire::ChatCompletions,
    OpenCodeGoWire::AnthropicMessages,
];

pub(super) const fn wire_name(wire: OpenCodeGoWire) -> &'static str {
    match wire {
        OpenCodeGoWire::Responses => "responses",
        OpenCodeGoWire::ChatCompletions => "chat_completions",
        OpenCodeGoWire::AnthropicMessages => "anthropic_messages",
    }
}

pub(super) fn select_rows(catalog: &[(ProviderModel, OpenCodeGoWire)]) -> Vec<LiveRow> {
    let mut selected = Vec::new();
    for wire in WIRES {
        let mut rows = catalog.iter().filter(|(_, candidate)| *candidate == wire);
        let preferred = rows
            .clone()
            .find(|(model, _)| {
                model.capabilities.tool_calls && model.capabilities.strict_structured_output
            })
            .or_else(|| rows.find(|(model, _)| model.capabilities.tool_calls));
        match preferred {
            Some((model, wire)) => selected.push(LiveRow {
                model: model.clone(),
                wire: *wire,
            }),
            None if catalog.iter().any(|(_, candidate)| *candidate == wire) => {
                eprintln!("OPENCODE_LIVE wire={} outcome=provider_capability_unsupported capability=tool_calls", wire_name(wire));
            }
            None => {
                eprintln!("OPENCODE_LIVE wire={} outcome=provider_model_unavailable capability=catalog_row", wire_name(wire));
            }
        }
    }
    let kimi = catalog.iter().find(|(model, wire)| {
        *wire == OpenCodeGoWire::ChatCompletions
            && (model.model.to_ascii_lowercase().contains("kimi")
                || model.display_name.to_ascii_lowercase().contains("kimi"))
    });
    if let Some((model, wire)) = kimi {
        if model.capabilities.tool_calls
            && !selected.iter().any(|row| row.model.model == model.model)
        {
            selected.push(LiveRow {
                model: model.clone(),
                wire: *wire,
            });
        } else if !model.capabilities.tool_calls {
            eprintln!("OPENCODE_LIVE wire=chat_completions model={} outcome=provider_capability_unsupported capability=tool_calls regression=kimi", model.model);
        }
    } else {
        eprintln!("OPENCODE_LIVE wire=chat_completions outcome=provider_model_unavailable capability=kimi_catalog_row");
    }
    selected
}
