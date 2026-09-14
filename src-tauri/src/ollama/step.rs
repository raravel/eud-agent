use super::*;

impl ProductionOllamaAdapter {
    pub(super) async fn execute_step(
        &mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> Result<AdapterStepOutcome, ProviderRuntimeError> {
        if request.binding.provider != ProviderId::Ollama {
            return Err(ProviderRuntimeError::InvalidBinding(
                "Ollama adapter received another provider".to_string(),
            ));
        }
        let model =
            validate_model(&request.binding.model).map_err(ProviderRuntimeError::InvalidBinding)?;
        validate_reasoning(request.binding.reasoning.as_ref())
            .map_err(ProviderRuntimeError::InvalidBinding)?;
        let base_url =
            normalize_base_url(request.binding.base_url.as_deref().ok_or_else(|| {
                ProviderRuntimeError::InvalidBinding(
                    "Ollama binding requires a base URL".to_string(),
                )
            })?)
            .map_err(ProviderRuntimeError::InvalidBinding)?;

        let (prompt, output_schema, tools_disabled) = match &request.kind {
            AdapterRequestKind::Foreground(turn) => (
                (!request.history.iter().any(|item| {
                    matches!(
                        item,
                        ConversationItem::User { request_id, .. }
                            if request_id == &request.identity.request_id
                    )
                }))
                .then_some((&turn.text[..], &turn.image_paths[..])),
                turn.output_schema.as_ref(),
                turn.forbid_tools,
            ),
            AdapterRequestKind::Structured {
                prompt,
                output_schema,
                ..
            } => (Some((&prompt[..], &[][..])), Some(output_schema), true),
        };
        let mut messages =
            conversation_messages(&request.history).map_err(ProviderRuntimeError::Protocol)?;
        if let Some((text, image_paths)) = prompt {
            messages.push(user_message(text, image_paths).map_err(ProviderRuntimeError::Protocol)?);
        }
        messages.extend(
            request
                .prior_tool_results
                .iter()
                .filter(|result| {
                    !request.history.iter().any(|item| matches!(
                item,
                ConversationItem::Assistant(NormalizedBlock::ToolResult { result: stored, .. })
                    if stored.id == result.id
            ))
                })
                .map(tool_result_message),
        );

        let structured = matches!(request.kind, AdapterRequestKind::Structured { .. })
            || (tools_disabled && output_schema.is_some());
        let tools = if tools_disabled || structured {
            Vec::new()
        } else {
            chat_tools(&request.tool_descriptors)
        };
        let body = build_chat_request(
            model,
            request.binding.reasoning.as_ref(),
            &messages,
            tools,
            selected_output_schema(structured, output_schema),
            request.policy.max_output_tokens,
        );
        let mut http_request = self
            .client
            .post(format!("{base_url}/chat/completions"))
            .json(&body);
        if let Some(api_key) = self
            .api_key
            .as_deref()
            .map(|key| key.trim())
            .filter(|key| !key.is_empty())
        {
            http_request = http_request.bearer_auth(api_key);
        }

        let mut cancellation = request.cancellation;
        let generation = request.identity.cancellation_generation;
        let send = http_request.send();
        tokio::pin!(send);
        let response = loop {
            tokio::select! {
                response = &mut send => break response
                        .map_err(|_| ProviderRuntimeError::Transport("provider_transport_closed".to_string()))?,
                changed = cancellation.changed() => {
                    match changed {
                        Ok(()) if *cancellation.borrow() != generation => {
                            return Ok(AdapterStepOutcome::Cancelled);
                        }
                        Ok(()) => continue,
                        Err(_) => {
                            return Err(ProviderRuntimeError::Transport(
                                "provider cancellation channel closed".to_string(),
                            ));
                        }
                    }
                }
            }
        };
        if !response.status().is_success() {
            let status = response.status();
            let error = status_error(status);
            return Err(if status.is_server_error() {
                ProviderRuntimeError::Transport(error)
            } else {
                ProviderRuntimeError::Protocol(error)
            });
        }

        let response_id = format!(
            "ollama-{}-{}",
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
        let mut step = match parse_chat_stream(
            response,
            &mut cancellation,
            generation,
            MAX_RESPONSE_BYTES,
            &events,
            &request.identity,
            &response_id,
        )
        .await
        {
            Ok(step) => step,
            Err(ProviderRuntimeError::Cancelled) => return Ok(AdapterStepOutcome::Cancelled),
            Err(error) => return Err(error),
        };

        if let Some(usage) = step.usage.as_mut() {
            usage.model_context_window = request
                .binding
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.context_window)
                .and_then(|window| i64::try_from(window).ok());
            send_event(
                &events,
                &request.identity,
                AdapterEventKind::Usage(normalized_usage(usage)),
            )
            .await?;
        }
        let batch_id = format!("{response_id}-tools");
        for call in &step.tool_calls {
            send_event(
                &events,
                &request.identity,
                AdapterEventKind::Block(NormalizedBlock::ToolCall {
                    response_id: response_id.clone(),
                    batch_id: batch_id.clone(),
                    call: call.clone(),
                    continuation: None,
                }),
            )
            .await?;
        }
        send_event(
            &events,
            &request.identity,
            AdapterEventKind::ResponseFinished {
                response_id,
                finish_reason: step.finish_reason.clone(),
                complete: true,
            },
        )
        .await?;

        if !step.tool_calls.is_empty() {
            if structured {
                return Err(ProviderRuntimeError::StructuredOutputInvalid);
            }
            return Ok(AdapterStepOutcome::NeedsTools {
                calls: step.tool_calls,
                continuation: None,
            });
        }
        if structured {
            let value = serde_json::from_str(&step.text)
                .map_err(|_| ProviderRuntimeError::StructuredOutputInvalid)?;
            return Ok(AdapterStepOutcome::Completed {
                output: AdapterOutput::Structured(value),
                continuation: None,
                native_conversation: None,
            });
        }
        Ok(AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(step.text),
            continuation: None,
            native_conversation: None,
        })
    }
}
