# eud-agent Design System

## 1. Atmosphere & Identity

eud-agent is a focused, dark Windows authoring workspace: calm enough for long sessions, dense enough for technical work, and explicit about risky filesystem operations. Its signature is emerald-accented state communication on layered deep-slate surfaces. Instructions should reduce uncertainty before the user opens a native picker.

## 2. Color

The implementation source of truth is `panel/src/index.css`; components use semantic Tailwind tokens rather than raw colors.

| Role | Token | Usage |
|---|---|---|
| App background | `background` | Full application shell |
| Primary text | `foreground` | Headings and body copy |
| Surface | `card`, `popover` | Cards, dialogs, menus |
| Secondary surface | `muted`, `secondary`, `accent` | Supporting rows and hover states |
| Secondary text | `muted-foreground` | Guidance and metadata |
| Primary action | `primary`, `primary-foreground` | Main action and current state |
| Error | `destructive` | Failure and recovery guidance |
| Boundary | `border`, `input`, `ring` | Separation, controls, focus |

Status success may use the existing emerald ramp already used by setup completion states. Accent color communicates action or state, never decoration.

## 3. Typography

- Primary: the WebView/system sans stack provided by Tailwind and Windows.
- Mono: the existing Tailwind monospace stack for code and paths.
- Scale: `text-xs` for metadata, `text-sm` for controls and body guidance, `text-base` for section titles, `text-lg` for dialog titles, and `text-2xl`/`text-3xl` for setup-page titles.
- Body guidance uses a relaxed line height. Korean instructions use `break-keep` where phrase grouping matters; filesystem paths use `break-all` or truncation with a visible full-value affordance.
- User-facing body text does not go below `text-sm`; `text-xs` is reserved for secondary metadata.

## 4. Spacing & Layout

- Base unit: 4px, expressed through Tailwind spacing steps.
- Compact gaps: 4–8px (`gap-1`, `gap-2`) for icon/label and status clusters.
- Standard gaps: 12–16px (`gap-3`, `gap-4`) for rows and grouped controls.
- Surface padding: 16–24px (`p-4`, `p-6`).
- Interactive targets are at least 44px high (`min-h-11` / `size-11`).
- Setup content is limited to `max-w-5xl`; focused forms and dialogs use `max-w-xl` or `sm:max-w-lg`.
- App dialogs use a bounded grid: header and footer remain visible; the content body is the sole vertical scroll owner and has `min-h-0`.
- At 375px, multi-column actions reflow to one readable column and unbroken paths must not create horizontal page scroll.

## 5. Components

### Button

- Structure: semantic `<button>` through the shared shadcn primitive.
- Variants: primary, outline, secondary, ghost, destructive, link.
- States: default, hover, active, visible keyboard focus, disabled, and in-place loading label/icon.
- Accessibility: descriptive accessible name, 44px target for primary workflows, no icon-only action without `aria-label`.

### Dialog

- Structure: Radix `Dialog` with labelled title, description, bounded content, and explicit close action.
- States: open/closed, keyboard focus trap, Escape close, disabled close during an irreversible in-flight step only.
- Layout: fixed overlay; dialog header/footer stay fixed while the named content body scrolls.
- Motion: existing 200ms opacity/transform transition, removed under reduced motion.

### Status or recovery notice

- Structure: icon, concise heading or lead sentence, then a concrete recovery action.
- Variants: neutral guidance, success, warning, destructive error.
- Accessibility: uses `role="status"` for progress and `role="alert"` for failures; color is never the only signal.

### E3S import step

- Structure: numbered state marker, step title, explanation, path selection button, and selected-path summary.
- States: pending, current, complete, invalid, and loading.
- Content: Step 1 selects the `.e3s`; Step 2 selects a new empty work folder; Step 3 confirms and imports.
- Error: preserve completed selections, identify the failed condition, and state the next recovery action.
- Accessibility: ordered semantic list, `aria-current="step"`, live progress, full keyboard operation, and focus returned to the triggering control on close.

### Card or grouped surface

- Structure: shared card or equivalent rounded semantic-token surface with heading, description, and actions.
- Depth: one subtle boundary plus restrained `shadow-sm` only where the surface is elevated.

## 6. Motion & Interaction

- Micro feedback: 150ms or the shared button transition.
- Dialog and category changes: 200ms opacity/transform only.
- Loading uses the existing spinner and changes the action label in place.
- Motion always communicates open/close, current step, or active work. Decorative motion is not used.
- `prefers-reduced-motion` removes nonessential animation without removing progress or state text.

## 7. Depth & Surface

Use a mixed but restrained depth strategy: semantic tonal shifts establish most hierarchy; one low-contrast border separates groups; `shadow-sm` marks cards and the existing dialog shadow marks overlays. Do not add raw shadow colors or decorative glows.

## 8. Accessibility Constraints & Accepted Debt

### Constraints

- Target WCAG 2.2 AA with 4.5:1 body-text and 3:1 large-text contrast.
- Every action is keyboard reachable and has a visible focus ring.
- Busy operations disable duplicate submission and expose text status in the same surface.
- Errors use plain Korean, name the cause category, and provide a recovery action.
- The import flow avoids memory-dependent instructions: both selections and their purpose remain visible until completion.
- Verify 375px, 768px, 1280px, 200% zoom, reduced motion, long Korean labels, and long unbroken Windows paths.

### Accepted Debt

None for the E3S import flow.
