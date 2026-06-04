# Decision 05: Monaco editor adopted for the edit tab (no-code-editor rule revoked)

- Date: 2026-06-04
- Status: Accepted
- Context: The original rules.md forbade code-editor components ("no Monaco/CodeMirror; textarea + server-side diff only") to keep the panel light. With the React rebuild, the user chose a full editor for the edit UX.
- Considered:
  - Monaco editor (npm-bundled) — Pros: real editing UX (highlight, indent, find), WebView2=Chromium is Monaco's native target. Cons: bundle size, worker wiring in Vite. Recommendation: presented as "rule fully revoked" option; chosen by user.
  - Read-only highlight (AI Elements CodeBlock) + textarea edit — Pros: keeps the lightweight rule intent. Cons: editing stays plain. Recommendation: ★★★ at ask time.
  - Status quo (no highlight, textarea) — Pros: nothing changes. Cons: rejects UX goal. Recommendation: ★☆☆.
- Chosen: Monaco editor for the edit tab. The diff tab continues to render the SERVER-supplied unified diff with +/- coloring (no Monaco DiffEditor — that would require the server to ship original file content, a WS protocol change out of scope).
- Rationale: user decision ("Monaco 도입 (룰 완전 폐기)").
- Impact: rules.md (the no-code-editor clause is REPLACED by Monaco-specific constraints: npm bundle only, `loader.config({ monaco })`, never the default CDN loader), features/03_agent-panel.md (edit tab spec), tech-stack.md (monaco-editor + @monaco-editor/react deps).
