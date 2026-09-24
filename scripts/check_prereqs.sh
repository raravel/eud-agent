#!/usr/bin/env bash
# Shared prerequisite checks for the macOS/Unix development scripts (source only).
#
# Mirror of scripts/check_prereqs.ps1. The Tauri/Rust app installs and gates its
# selected provider at runtime, so normal build/dev scripts request no provider
# prerequisite. These opt-in CLI probes serve provider-specific live smoke only:
#   - codex: CODEX_CMD, then PATH
#   - claude-code: CLAUDE_CODE_CMD, then PATH
#
# Direct Antigravity/OpenCode Go smoke is credential-driven inside the app and
# deliberately has no executable prerequisite.
#
# Defines functions only -- no work happens at source time.

resolve_codex_cmd() {
    if [[ -n "${CODEX_CMD:-}" ]]; then
        printf '%s\n' "$CODEX_CMD"
        return 0
    fi
    command -v codex 2>/dev/null
}

resolve_claude_code_cmd() {
    if [[ -n "${CLAUDE_CODE_CMD:-}" ]]; then
        printf '%s\n' "$CLAUDE_CODE_CMD"
        return 0
    fi
    command -v claude 2>/dev/null
}

# Usage: prereq_failures codex claude-code
# Prints one message per failure; empty output means every requested check passed.
prereq_failures() {
    local requirement resolved
    for requirement in "$@"; do
        case "$requirement" in
            codex)
                resolved="$(resolve_codex_cmd || true)"
                if [[ -z "$resolved" ]]; then
                    echo "codex CLI not found for live smoke (checked CODEX_CMD, then PATH). Install it with 'brew install codex' or npm."
                elif [[ ! -f "$resolved" ]]; then
                    echo "codex: resolved path does not exist: '$resolved'"
                fi
                ;;
            claude-code)
                resolved="$(resolve_claude_code_cmd || true)"
                if [[ -z "$resolved" ]]; then
                    echo "Claude Code CLI not found for live smoke (checked CLAUDE_CODE_CMD, then PATH). Install it from the app's AI provider settings."
                elif [[ ! -f "$resolved" ]]; then
                    echo "claude-code: resolved path does not exist: '$resolved'"
                fi
                ;;
            *)
                echo "unknown prerequisite: $requirement"
                ;;
        esac
    done
}
