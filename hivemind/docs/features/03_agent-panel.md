# Agent panel

## Surface

The React/TypeScript panel is hosted by the standalone Tauri WebView2 window. It communicates only through typed Tauri `invoke` commands and `listen` events; there is no localhost server or Editor-hosted browser.

Primary regions:

- persistent multi-session sidebar;
- header with project identity, in-process transport, RAG, Map Agent, settings, and project tools;
- conversation/stream/ASK/plan/changeset review;
- instruction composer with attachments, mentions, model controls, and cancel;
- project sidebar for workspace, DAT wiki, and memory;
- first-run setup overlay;
- settings categories for Project, Notifications, and Codex.

## Project states

`connected` means Tauri listeners are registered. `projectAvailable` means the configured canonical project can be opened. These states are independent.

- unavailable project: notice names `project.json`, source path, and recovery action; sending is gated;
- available project with source list: normal authoring;
- build marker active: mutating paths are busy-gated by backend state;
- periodic native status refresh can recover after external path restoration without reconnecting IPC.

No Editor launch button, Editor connection chip, Editor path picker, or heartbeat state exists.

## Setup and project management

First-run ordered steps:

1. open existing Native project, create from SCX/SCM, or import E3S;
2. choose euddraft executable/source entrypoint;
3. verify/download managed assets;
4. authenticate Codex.

The project step has one primary open action and two explicit secondary create/import actions. Async actions disable all competing project buttons and show a busy label. Settings → Project exposes the same switch/create/import actions plus E3S export.

## Conversation contracts

- All conversation events are scoped to immutable session ids.
- ASK answers resume the blocked tool call without a new chat turn.
- Plans and changesets are durable review surfaces.
- Another session remains usable while one session reads/reviews; writes serialize in the backend.
- Mention snapshots carry project/hash authority and are invalidated on project/scope change.
- Raw internal event kinds never appear as the only user-facing text.

## Accessibility and visual rules

- Korean labels throughout.
- Semantic buttons, navigation, headings, dialog roles, progressbars, and status regions.
- Keyboard operation and visible focus for project/settings/session/review actions.
- Lucide icons only; semantic theme tokens; no structural emoji.
- Minimum 44px project action targets and 8px gaps.
- Busy/error state is text plus icon/status, never color alone.
- Animation uses existing 150–300ms transitions and `motion-reduce` fallbacks.

## Verification

- `panel/src/App.test.tsx`
- `panel/src/setup/SetupScreen.test.tsx`
- `panel/src/components/SettingsDialog.test.tsx`
- `panel/src/lib/ipc.test.ts`
- complete 502-test Vitest suite, TypeScript build, production Vite build, and browser/Tauri surface acceptance.
