# Decision 02: NEWEPS returns ERROR on duplicate filename
- Date: 2026-06-04
- Status: Accepted
- Context: The new NEWEPS bridge command creates an eps file from a caller-provided filename. The verified PANEL chain avoided collisions with an auto-counter; switching to caller-provided names introduces an unspecified duplicate case (adversarial review finding).
- Considered:
  - Return ERROR — Pros: explicit and predictable; no surprise filenames; the panel surfaces the error and the user/agent picks a new name. Cons: one extra user step on collision. Recommendation: ★★★ — safer than inventing names.
  - Auto-suffix (name2, name3, …) — Pros: never fails. Cons: silently creates names the user did not choose; clutters the project tree. Recommendation: ★★☆.
- Chosen: Return ERROR
- Rationale: Explicit failure beats silent renaming for project-tree hygiene; user confirmed the recommendation.
- Impact: features/00_lua-bridge.md (NEWEPS spec), architecture.md (IPC protocol table)
