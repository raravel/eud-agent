---
task_id: EUD-071-eae9
completed_at: 2026-06-05T20:40:00
duration_minutes: 30
coding_retries: 1
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 50000
  output: 9000
cost_usd: 1.80
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-probe-verified
---

## Summary

User report: the eud-agent codex threads load the operator's PERSONAL codex
skills — the live E2E rollout showed **hv-clarify injected with its "MUST be
invoked BEFORE any implementation" instruction** (would derail map-edit turns
into a 7-axis questionnaire). This was the EUD-062 "skills cannot be isolated"
documented limitation; it is now CLOSED.

**Probe-driven resolution** (3 live probes):
1. Path-keyed `skills.config=[{path, enabled=false}]` enumeration (the
   documented schema) — **IGNORED** both as `-c` override and as per-thread
   config (model still listed the full catalog). Dead end recorded.
2. `skills.include_instructions=false` (found via openai/codex #20210) —
   model reports **NONE** (entire skill instruction block removed, personal +
   system skills).
3. Name-keyed `skills.config=[{name, enabled=false}]` — works per-skill
   (usable later via `CodexIsolation.extra_overrides` if selective filtering
   is wanted).

**Fix**: `CodexIsolation.disable_skills=True` (default) emits
`skills.include_instructions=false` in the launch overrides. The first-cut
enumeration implementation was replaced after probe 1 falsified it.

## Changes

`server/eud_agent/agent_runner.py` (isolation knob + module docstring
retiring the EUD-062 limitation), `server/tests/test_agent_flow.py` (2 tests),
features/05 doc.

## Verification

Verify-first: tests red (no override / no knob), green after. Live probes as
above (the deciding evidence). Full server suite 532 passed; ruff clean.

## Harness Sync

features/05 gained the "No skills (EUD-071)" paragraph; agent_runner module
docstring updated (EUD-062 honest-limitation paragraph replaced).

## Notes

- Why the global CLAUDE.md question turned out to be a codex issue: the hv
  plugin installs its skills for BOTH Claude Code and codex
  (`~/.codex/skills/hv-*`); codex loads them into every thread regardless of
  project, and the eud-agent threads inherited them.
