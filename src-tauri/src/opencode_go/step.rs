use super::*;

impl OpenCodeGoAdapter {
    pub(super) async fn run_one_step(
        &mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> Result<AdapterStepOutcome, ProviderRuntimeError> {
        if request.binding.provider != ProviderId::OpencodeGo {
            return Err(ProviderRuntimeError::InvalidBinding(
                "OpenCode Go adapter received another provider".to_string(),
            ));
        }
        if let Some(continuation) = &request.continuation {
            continuation.validate(ProviderId::OpencodeGo)?;
        }
        let generation = request.identity.cancellation_generation;
        let mut cancellation = request.cancellation.clone();
        if *cancellation.borrow() != generation {
            return Ok(AdapterStepOutcome::Cancelled);
        }
        let resolved = tokio::select! {
            result = self.resolve_model(request.identity.run_id, &request.binding.model) => result,
            changed = cancellation.changed() => {
                if changed.is_ok() && *cancellation.borrow() != generation {
                    return Ok(AdapterStepOutcome::Cancelled);
                }
                return Err(ProviderRuntimeError::Cancelled);
            }
        };
        let (api_key, model) = resolved?;
        if let Some(continuation) = &request.continuation {
            if continuation.data.get("wire").and_then(Value::as_str) != Some(wire_name(model.wire))
            {
                return Err(ProviderRuntimeError::ContinuationInvalid);
            }
        }
        let structured = matches!(&request.kind, AdapterRequestKind::Structured { .. });
        let (history, output_schema, forbid_tools) = runtime_history(&request)?;
        if !structured
            && history
                .iter()
                .all(|entry| !matches!(entry, WireHistory::User { .. }))
        {
            return Err(ProviderRuntimeError::Protocol(
                "provider request has no user turn".to_string(),
            ));
        }
        let tools = match &output_schema {
            Some(schema) => {
                if !model.structured_output {
                    return Err(ProviderRuntimeError::Protocol(
                        "provider_capability_unsupported".to_string(),
                    ));
                }
                vec![structured_tool(schema, model.wire)]
            }
            None if forbid_tools => Vec::new(),
            None => {
                if !model.tool_calls && !request.tool_descriptors.is_empty() {
                    return Err(ProviderRuntimeError::Protocol(
                        "provider_capability_unsupported".to_string(),
                    ));
                }
                wire_tools(&request.tool_descriptors, model.wire)
            }
        };
        let output_tokens = match (model.max_output_tokens, request.policy.max_output_tokens) {
            (Some(known), Some(requested)) => Some(known.min(requested)),
            (known, None) => known,
            (None, requested) => requested,
        };
        let mut body = build_runtime_request(
            model.wire,
            output_tokens,
            &request.binding.model,
            &history,
            tools,
            structured,
        )
        .map_err(ProviderRuntimeError::Protocol)?;
        if structured {
            add_structured_instructions(&mut body, model.wire)?;
        }

        let response_id = format!(
            "{}-{}",
            request.identity.run_id.get(),
            request.history.len()
        );
        send_event(
            &events,
            &request.identity,
            AdapterEventKind::ResponseStarted {
                response_id: response_id.clone(),
            },
        )
        .await?;
        let http_request = authenticated_request(
            &self.client,
            format!(
                "{}/{}",
                self.inference_base_url.trim_end_matches('/'),
                wire_path(model.wire)
            ),
            model.wire,
            &api_key,
            &request.identity.session_id,
        )
        .json(&body);
        let response = tokio::select! {
            sent = http_request.send() => sent.map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".to_string()))?,
            changed = cancellation.changed() => {
                if changed.is_ok() && *cancellation.borrow() != generation {
                    return Ok(AdapterStepOutcome::Cancelled);
                }
                return Err(ProviderRuntimeError::Cancelled);
            }
        };
        let status = response.status();
        if !status.is_success() {
            return Err(ProviderRuntimeError::Transport(status_error(status)));
        }
        let parsed = match parse_stream_events(
            response,
            StreamEventRequest {
                wire: model.wire,
                cancellation: &mut cancellation,
                generation,
                events: &events,
                identity: &request.identity,
                response_id: &response_id,
            },
        )
        .await
        {
            Ok(parsed) => parsed,
            Err(error) => {
                let mapped = map_stream_error(error);
                if matches!(&mapped, ProviderRuntimeError::Transport(_)) {
                    let _ = send_event(
                        &events,
                        &request.identity,
                        AdapterEventKind::TransportClosed,
                    )
                    .await;
                }
                return Err(mapped);
            }
        };
        let mut step = parsed.step;
        if let Some(usage) = step.usage.as_mut() {
            usage.model_context_window = model
                .context_window
                .and_then(|window| i64::try_from(window).ok());
        }
        let continuation = parsed.continuation;
        for block in parsed.deferred_blocks {
            send_event(&events, &request.identity, AdapterEventKind::Block(block)).await?;
        }
        if let Some(usage) = step.usage.as_ref().map(normalized_usage) {
            send_event(&events, &request.identity, AdapterEventKind::Usage(usage)).await?;
        }
        let complete = response_is_complete(&step, structured);
        send_event(
            &events,
            &request.identity,
            AdapterEventKind::ResponseFinished {
                response_id,
                finish_reason: step.finish_reason.clone(),
                complete,
            },
        )
        .await?;
        if !complete {
            return Err(ProviderRuntimeError::Protocol(
                "provider returned an incomplete response".to_string(),
            ));
        }

        if structured {
            let schema = output_schema
                .as_ref()
                .ok_or(ProviderRuntimeError::StructuredOutputInvalid)?;
            let value = structured_value(&step.tool_calls, schema)?;
            return Ok(AdapterStepOutcome::Completed {
                output: AdapterOutput::Structured(value),
                continuation,
                native_conversation: None,
            });
        }
        if !step.tool_calls.is_empty() {
            return Ok(AdapterStepOutcome::NeedsTools {
                calls: step.tool_calls,
                continuation,
            });
        }
        Ok(AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(step.text),
            continuation,
            native_conversation: None,
        })
    }
}
