use crate::provider_tool_loop::{
    clear_native_run_recovery, unresolved_native_runs, NativeRunReceiptState,
};

use super::{ProviderConversationState, ProviderRuntime, ProviderRuntimeError};
use crate::provider::ProviderId;

impl ProviderRuntime {
    pub(super) async fn recover_native_conversation(
        &mut self,
        identity: &super::super::RunIdentity,
    ) -> Result<(), ProviderRuntimeError> {
        let (conversation, acknowledgements) = self
            .resolve_native_conversation(self.conversation.clone(), Some(&identity.request_id))?;
        if conversation != self.conversation {
            self.adapter.seed(conversation.clone()).await?;
            self.conversation = conversation.clone();
            self.binding.conversation = conversation;
        }
        self.remember_native_acknowledgements(acknowledgements);
        Ok(())
    }

    pub(super) fn resolve_native_conversation(
        &self,
        mut conversation: ProviderConversationState,
        request_id: Option<&str>,
    ) -> Result<(ProviderConversationState, Vec<super::super::RunIdentity>), ProviderRuntimeError>
    {
        let receipts = unresolved_native_runs(&self.dirs.journal_dir(), self.tools.session_id())
            .map_err(ProviderRuntimeError::Transport)?;
        let mut completed = Vec::new();
        for receipt in receipts {
            if receipt.provider != self.binding.provider {
                continue;
            }
            match receipt.state {
                NativeRunReceiptState::Pending | NativeRunReceiptState::Unknown => {
                    return Err(ProviderRuntimeError::Protocol(
                        "이전 네이티브 실행의 완료를 확인할 수 없습니다. 대화를 초기화한 뒤 다시 요청해 주세요.".into(),
                    ));
                }
                // An interrupted run did not finish its turn, but the session it
                // named stays resumable: continue from it instead of refusing the
                // conversation. Without a candidate it is no better than unknown.
                NativeRunReceiptState::Interrupted if receipt.candidate_native_id.is_none() => {
                    return Err(ProviderRuntimeError::Protocol(
                        "이전 네이티브 실행의 완료를 확인할 수 없습니다. 대화를 초기화한 뒤 다시 요청해 주세요.".into(),
                    ));
                }
                NativeRunReceiptState::Completed | NativeRunReceiptState::Interrupted => {
                    completed.push(receipt);
                }
                NativeRunReceiptState::Cleared => {}
            }
        }
        let mut confirmed_ids = vec![conversation.conversation_key()];
        let mut acknowledgements = Vec::new();
        while !completed.is_empty() {
            let current_id = conversation.conversation_key();
            let Some(index) = completed.iter().position(|receipt| {
                receipt.prior_native_id == current_id
                    || confirmed_ids.contains(&receipt.candidate_native_id)
            }) else {
                return Err(ProviderRuntimeError::Protocol(
                    "저장된 네이티브 실행 경계가 현재 대화와 다릅니다. 대화를 초기화한 뒤 다시 요청해 주세요.".into(),
                ));
            };
            let receipt = completed.remove(index);
            // Re-running a settled request would replay it; resuming the request
            // an interruption cut short is exactly what the next run is for.
            if receipt.state == NativeRunReceiptState::Completed
                && Some(receipt.identity.request_id.as_str()) == request_id
            {
                return Err(ProviderRuntimeError::Protocol(
                    "이미 완료된 요청의 저장 복구가 필요합니다. 같은 요청을 자동으로 재실행할 수 없습니다.".into(),
                ));
            }
            let candidate = receipt.candidate_native_id.clone().ok_or_else(|| {
                ProviderRuntimeError::Protocol("native completed receipt has no checkpoint".into())
            })?;
            if !confirmed_ids.contains(&receipt.candidate_native_id) {
                conversation = match self.binding.provider {
                    ProviderId::Codex => ProviderConversationState::Codex {
                        thread_id: Some(candidate),
                    },
                    ProviderId::ClaudeCode => ProviderConversationState::ClaudeCode {
                        session_id: Some(candidate),
                    },
                    ProviderId::Antigravity | ProviderId::OpencodeGo | ProviderId::Ollama => {
                        return Err(ProviderRuntimeError::Protocol(
                            "direct provider has a native receipt".into(),
                        ));
                    }
                };
                confirmed_ids.push(receipt.candidate_native_id);
            }
            confirmed_ids.push(receipt.prior_native_id);
            acknowledgements.push(receipt.identity);
        }
        Ok((conversation, acknowledgements))
    }

    pub(super) fn remember_native_acknowledgements(
        &mut self,
        acknowledgements: Vec<super::super::RunIdentity>,
    ) {
        for identity in acknowledgements {
            if !self.pending_acknowledgements.contains(&identity) {
                self.pending_acknowledgements.push(identity);
            }
        }
    }

    pub(super) fn clear_native_recovery(&mut self) -> Result<(), ProviderRuntimeError> {
        let directory = self.dirs.journal_dir();
        clear_native_run_recovery(
            &directory,
            self.tools.session_id(),
            self.binding.provider,
            self.conversation.conversation_key().as_deref(),
        )
        .map_err(ProviderRuntimeError::Transport)?;
        let receipts = unresolved_native_runs(&directory, self.tools.session_id())
            .map_err(ProviderRuntimeError::Transport)?;
        for receipt in receipts {
            if receipt.provider == self.binding.provider {
                clear_native_run_recovery(
                    &directory,
                    self.tools.session_id(),
                    self.binding.provider,
                    receipt.prior_native_id.as_deref(),
                )
                .map_err(ProviderRuntimeError::Transport)?;
            }
        }
        self.pending_acknowledgements.clear();
        Ok(())
    }
}
