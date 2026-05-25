# Rune Editor Workpackages

This directory is the execution plan for implementing the Rune editor from `editor-spec.md`. A worker should treat these files as the contract of record for implementation tasks, with `CLAUDE.md`, `qa-instructions.md`, and `qa-implementation-specs.md` as mandatory supporting inputs.

## Required Reading

Before starting any workpackage, read:

1. `CLAUDE.md` — repository engineering rules. These override local shortcuts.
2. `editor-spec.md` — editor behavior and architecture.
3. `qa-instructions.md` — testing authority for `pkg/editor/`, `pkg/command/`, and `pkg/ui/components/editor/`.
4. `qa-implementation-specs.md` — compound risk scenarios and acceptance examples.
5. This README and the assigned workpackage.

When a workpackage conflicts with one of the files above, stop and fix the workpackage or ask for clarification. Do not silently reinterpret the spec.

## Global Worker Contract

- Use value semantics. `Init`, `Update`, and `View` on models use value receivers.
- Do not store `context.Context` or `*log.Logger` on models.
- Do not silently discard, replace, or corrupt user data. Invalid file data and invalid edit batches surface hard errors.
- Keep `pkg/ui/keymap/keymap.go` as the single physical-key source of truth.
- Keep `pkg/editor/...` domain packages headless. They must not import UI components or pages.
- Keep `pkg/editor/display` semantic. Lipgloss styling belongs in the UI editor renderer, not the domain display pipeline.
- Capture model-derived values into locals before creating `tea.Cmd` closures.
- Do not wrap synchronous state transitions in `tea.Cmd` messages.
- Accumulate every child `tea.Cmd` returned from child `Update` calls.
- Keep Go files under 500 LoC. Split by cohesive responsibility before a file grows large.
- Tests must validate spec behavior, not private struct layout.
- `internal/editortest` must use only primitive data types and must not import editor packages.

## Execution Order

| Stage | Workpackages | Notes |
|---|---|---|
| Preflight | WP00, WP01, WP05, WP06 | Can run mostly in parallel. Fix key collision checks before editing keys land. |
| Core domain | WP02, WP03, WP04, WP07 | Buffer, cursor, history, and display pipeline. |
| Editor shell | WP08 | Split internally into state machine and view/wiring phases. |
| Raw editing | WP09, WP10, WP11, WP12 | Navigation, editing, multi-cursor, undo/redo. |
| App safety | WP13 | Data-loss guards, footer/opentabs routing, consumed-key handling. |
| Markdown | WP14a, WP14b, WP14c, WP14d | Suffix split of the original WP14 scope. |
| Interaction | WP15, WP16, WP17 | Clipboard, mouse, terminal/image support. |
| Deferred UX | WP18 | MVP stub only unless the full Phase 2 package is explicitly approved. |
| Final gates | WP19 | Build, tests, spec coverage, docs consistency, manual smoke. |

## Split Packages

Use suffix package files to reduce risk without renumbering everything:

- `wp08-editor-component.md` contains WP08A/WP08B phases rather than a single monolithic rewrite.
- `wp14-markdown-preview.md` is the parent scope. Implement the suffix files for actual markdown preview work:
  - `wp14a-markdown-inline-line.md`
  - `wp14b-markdown-blocks.md`
  - `wp14c-markdown-advanced.md`
  - `wp14d-code-fence-highlighting.md`

## Per-Workpackage Checklist

Each workpackage should be executable at fill-in-the-blanks level and include:

- Inputs to read.
- Current repository state relevant to the task.
- Exact files and symbols to add or modify.
- Non-goals and deferred scope.
- Invariants that must hold after implementation.
- Table-driven tests and any property/fuzz tests.
- Verification commands.
- Handoff notes for downstream workpackages.

## Shared Verification

Run the package-specific verification first. Before handing off a completed phase, also run the narrowest safe project-level command available:

```bash
go test ./pkg/editor/... ./pkg/command/... ./internal/editortest/...
go test ./pkg/ui/components/editor/... ./pkg/ui/pages/workspace/... ./pkg/ui/keymap/...
go build ./...
```

When a command cannot run because downstream packages do not exist yet, state that explicitly in the handoff notes and run the closest package-level command that can compile.

## Known Pitfalls To Reject

- Non-error `ParseState` signatures in new docs or code. The current helper returns `(TestState, error)`.
- Dirty tracking based only on a saved buffer-version counter. Undo-back-to-saved must become clean.
- A single massive command implementation file edited by every feature package.
- UI style types in `pkg/editor/display` domain snippets.
- Old navigation counts for the current navigation table. It has 16 navigation/scroll commands.
- Full find/replace implementation hidden under a “Phase 2 — stubbed” label.
- Backspace bound to footer help while editor editing is enabled.
