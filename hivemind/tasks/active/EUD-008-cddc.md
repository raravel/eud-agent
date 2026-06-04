---
created: '2026-06-04'
depends_on:
- EUD-007-761d
id: EUD-008-cddc
parent: EUD-002-684a
priority: high
scope:
- bridge/**
- vendor/**
- server/eud_agent/**
status: pending
title: Import verified artifacts (bridge v6, runner, WebView2 DLLs)
type: task
updated: '2026-06-04'
---

## Description
Import the verified external artifacts unchanged (import-then-extend rule): ZZZ_10_agent_bridge.lua v6 from C:\Users\ifthe\eud-agent-analysis\test-lua\ into bridge/; eud_agent_runner.py from C:\Users\ifthe\proj\eud\ECA\ into server/eud_agent/ as runner_legacy.py reference; the 3 WebView2 SDK DLLs (Core, Wpf, Loader x64, SDK 1.0.3800.47) from the editor install into vendor/webview2/.

## Spec References
- [[tech-stack]] `../docs/tech-stack.md` - Legacy / Vendored
- [[features/01_lua-bridge|01_lua-bridge]] `../docs/features/01_lua-bridge.md`

## Completion Criteria
- [ ] bridge/ZZZ_10_agent_bridge.lua byte-identical to the verified v6 source
- [ ] vendor/webview2/ has the 3 DLLs with sizes matching tech-stack.md (649840 / 82544 / 160880 bytes)
- [ ] runner draft imported for reference; original files untouched at their source locations
- [ ] ECA and eud-agent-analysis repos unmodified