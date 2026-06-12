# Rune

A Bubble Tea (v2) TUI markdown editor and note-taking app built in Go.

## Language

**textedit**:
The base TUI text-editing component. A reusable bubbles-level primitive that owns buffer, cursors, history, viewport, and generic cell rendering. Has no opinion about what is being edited.
_Avoid_: editor (ambiguous), base editor, text input, textarea

**markdownedit**:
The rich markdown editing component that extends textedit with markdown parsing, reveal/render, syntax highlighting, image rendering, and link resolution. Composes textedit.Model by value and plugs in a markdown SyncFunc.
_Avoid_: rich editor, full editor, markdown editor

**SyncFunc**:
The single seam between textedit and its rendering extension. A pure function `func(buf, cursors, focused, width) SyncResult` that produces the full display pipeline output. Injected via `WithSyncFunc()` at construction time.
_Avoid_: pipeline hook, render callback, sync callback

**SyncResult**:
The output of a SyncFunc. Carries DisplaySnapshot, WrapSnapshot, and SyntaxSnapshot — everything the base editor needs for scrolling, coordinate conversion, and cell rendering.
_Avoid_: pipeline result, render output

**StyleHint**:
A `lipgloss.Style` embedded on `display.DisplaySpan`. The SyncFunc pre-computes each span's visual style; the View renders cells generically using StyleHint with zero TokenKind dispatch.
_Avoid_: span style, cell style, render style

**PlainSync**:
The default SyncFunc. Produces a trivial SyntaxSnapshot (all TokenText, all Revealed), wraps it, builds the snapshot. No parsing, no highlighter. Used by title and chat instances.
_Avoid_: default sync, text sync, identity sync

**MarkdownSync**:
The markdown SyncFunc. Parses the buffer via goldmark + advanced inlines, computes reveal/render with cursor awareness, builds CellMaps and coordinate deltas, applies Chroma highlighting as per-token StyleHint, and produces a fully styled SyncResult.
_Avoid_: rich sync, md sync, editor sync

**Extension**:
A SyncFunc that adds rendering intelligence to textedit. Not a Go interface — a concrete function value. Extensions mutate the buffer via textedit's stable public API (Insert, Delete, Replace), and those mutations flow through history/undo naturally.
_Avoid_: plugin, addon, module

### Component Instances

**title**:
A textedit instance configured with PlainSync, single-line constraint, no viewport scrolling. Used for file name editing.
_Avoid_: title editor, filename input, name field

**chat input**:
A textedit instance configured with PlainSync, single-line constraint, no viewport scrolling. Used for the AI chat pane's text input.
_Avoid_: chat editor, chat field, message input, chat box

**main editor**:
An markdownedit instance (textedit + MarkdownSync + image lifecycle). The full-featured editing surface in the center pane.
_Avoid_: content editor, rich editor, workspace editor

### Rendering Pipeline

**display pipeline**:
The transformation from buffer content to a renderable DisplaySnapshot: buffer → SyncFunc → SyncResult. textedit owns the pipeline frame; the SyncFunc fills in the content.
_Avoid_: render pipeline, sync pipeline, view pipeline

**cell rendering**:
The generic span→cell→overlay→string rendering in textedit.View(). Converts StyleHint-bearing DisplaySpans into cells, applies cursor/selection overlays, handles horizontal scroll via sliceCells, and stringifies.
_Avoid_: view rendering, span rendering, output rendering

## Boundaries

### textedit owns
- `buffer.Buffer`, `cursor.CursorSet`, `history.UndoStack`
- Command system (insert, delete, newline, clipboard, undo/redo, navigation, multi-cursor, mouse)
- Viewport state and scrolling (`scrollToCursor`, `scrollToBottom`)
- `SetSize(w,h)`, `Height()`, `SetFocused(bool)`, `SetReadOnly(bool)`
- Generic cell rendering (span→cell→overlay→string)
- `syncDisplay()` — calls SyncFunc, stores resulting SyncResult
- `WrapMap` — soft wrapping is generic; the SyncFunc produces a SyntaxSnapshot, wrapping follows
- `SanitizeFunc` — `func(text string) string` pre-insert hook, injected via constructor

### markdownedit owns (on top of textedit)
- Markdown SyncFunc (goldmark parsing, reveal/render, CellMap, coordinate deltas)
- Code highlighting (Chroma) — expressed as per-token StyleHint pre-baked into sub-spans
- Table layout and expansion (adaptive grid/wrapped/pivoted, border generation)
- Image lifecycle (discovery, allocation, decode, placement, animation, cleanup)
- Link click resolution and LinkClickedMsg emission
- FrontmatterMode setting and YAML validation

### workspace owns
- File I/O (load, save, rename)
- Title textedit instance (sibling of markdownedit, not nested)
- Breadcrumb component
- Layout orchestration: title → markdownedit → breadcrumb overlay

### chat owns
- A read-only `markdownedit` for the conversation display (upper area)
- A `textedit` for prompt input (lower area), PlainSync, dynamic height

### Implementation decisions

**textedit and markdownedit** live at `pkg/ui/components/textedit/` and `pkg/ui/components/markdownedit/` respectively (both Bubble Tea components, not domain primitives).

**Constructor pattern** — functional options (`textedit.New(keys, styles, opts...)` and `markdownedit.New(keys, styles, termCaps, opts...)`).

**SanitizeFunc** — `func(text string) string` pre-insert hook on textedit that filters invalid characters before they reach the buffer. Title uses it to strip `<>!` etc.

**ReadOnly mode** — `SetReadOnly(bool)` on textedit gates all buffer-mutating commands as no-ops while keeping navigation, scroll, and copy working. SyncFunc still runs normally. Cursor is hidden. Used by chat display.

**Chat submit** — Shift+Enter submits the prompt; Enter inserts a newline in the prompt textedit.

**Dynamic height** — textedit exposes `ContentHeight(width int) int` for natural height queries. Parent (chat) clamps: `clamp(raw, min, max)` and allocates via `SetSize`.

**Command/API split** — textedit owns editing, navigation, multi-cursor, clipboard, history/undo, mouse. markdownedit adds images, link resolution, code highlighting (via SyncFunc). File commands and dictation are workspace/separate-omponent concerns.