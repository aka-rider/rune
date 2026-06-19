# Rune — Engineering Standard & Code Convention

These instructions are the **single source of truth** for all code produced in this repository.
Every rule uses imperative language. "MUST" means mandatory. "NEVER" means prohibited.
If code violates any rule below, it is defective and must be fixed before merge.

`rune` is a Bubble Tea (v2) TUI markdown editor and note-taking app built in Go.

---

## 0. Prime Directive — Protect the User's Words

`rune` holds the user's notes: their thinking, made durable. Every other rule in this document is subordinate to one: **never corrupt and never lose what the user has written.** When any concern — performance, elegance, architecture, even feature correctness — conflicts with data safety, data safety wins. A feature that puts the user's text at risk is not shipped; it is redesigned until it is safe.

### 0.1 The Harm Ladder

Rank every change, bug, and trade-off against this ladder. A defect's severity is the highest rung it can reach:

1. **Catastrophic — silent corruption or degradation.** Writing wrong or garbled bytes, mangling UTF-8, dropping or reordering content, silently rewriting the user's text (line endings, trailing newline, encoding, BOM), or overwriting a good file with a bad buffer. Worst of all because the user may not notice until the original is gone. This NEVER ships. Code that *might* do this does not merge.
2. **Severe — losing more than a few seconds of work.** A crash, failed save, or botched recovery that discards unsaved edits. The bar is explicit: **at most a few seconds of work may ever be at risk.** Anything that can lose minutes of typing is a Severe defect.
3. **Tolerable — everything else.** A render glitch, a wrong layout, a dropped keypress, even a clean crash that loses nothing. Fix them, but they do not threaten the user's data. It is always correct to accept a Tolerable failure — including halting an operation with a visible error — to avoid a Severe or Catastrophic one.

### 0.2 Fail Loud, Fail Safe, Never Silent

When you suspect data is at risk, **STOP and surface a hard error** (§1.3). A controlled halt that leaves the in-memory buffer intact is a Tolerable failure and is always preferable to the rungs above. But "stop" means a surfaced error — NOT a `panic`. A panic kills the process and takes the unsaved buffer with it (Severe). Stop without crashing, keep the buffer, tell the user.

### 0.3 Robustness Is Data Safety Under Stress

Malformed input, enormous files, exotic Unicode, a file changed underneath you, a full disk — all are inevitable, not edge cases. The editor MUST absorb them by degrading gracefully (show an error, refuse the operation, preserve the buffer), never by corrupting data or crashing with unsaved work. The rules that enforce this are concentrated in §1.3 (fail fast on data risk), **§1.4 (persistence & durability)**, §1.5 / `pkg/ui/CLAUDE.md` §4.5 (Unicode), and `pkg/ui/CLAUDE.md` §5 (the Elm cycle). Read §1.4 before touching any code that writes to disk, mutates the buffer, or closes a document.

---

## 1. Go Fundamentals

### 1.1 Value Semantics
- Use Go values over pointers. Only use pointers when a struct contains a `sync.Mutex` or owns a unique OS resource (file handle, socket).
- Models own their child models **by value**. NEVER store pointers to other models.
- Zero values MUST be meaningful. Prefer `type Result struct { Data T; Valid bool }` over `*T` with nil checks.

### 1.2 Concurrency
- Share memory by communicating. Data crossing goroutine boundaries MUST be passed by copy or be strictly immutable.
- NEVER access a Model field from inside a `tea.Cmd` closure. Copy the needed value into a local variable first.

### 1.3 Error Handling
- **Fail fast on data risk.** If there is ANY suspicion that user data may be corrupted or lost, STOP operations immediately and surface a hard error to the user.
- **Validate before you persist a destructive change.** An edit that collapses a non-empty region to empty, deletes a large fraction of the buffer, or shrinks a document far below what was loaded is *suspect until proven intentional*. Edits arriving from asynchronous or external sources (dictation, paste, IME composition, network, file watchers) are especially suspect: an upstream *reset* (e.g. an empty interim result) MUST NOT be applied as a user *deletion*. Drop or clamp such an edit; never let it reach the buffer — and therefore never let it reach disk.
- **Clamp ranges to the live buffer.** Before any `ReplaceRange(start, end, …)` / delete / replace, clamp `[start, end]` to the current buffer length. If the range is stale (`end` past the end of the buffer), cancel the operation rather than mutate a region that no longer exists. Offsets computed against an old revision are a corruption vector.
- **NEVER swallow errors.** Bare `_, _ = operation()` is prohibited unless annotated with `// fire-and-forget: <reason>`.
- **Contextual wrapping.** Always wrap errors with operation + resource context: `fmt.Errorf("load dir %q: %w", dir, err)`. Always use `%w`, never `%s` or `%v` — `%w` preserves the error chain for `errors.Is`/`errors.As`.
- **Never downgrade `error` to `string`.** When returning or storing an error, always use the `error` type and always chain the original error with `%w`. Converting to a string with `%s`/`%v` or `fmt.Sprintf` severs the chain and prevents `errors.Is`/`errors.As` from working.

```go
// WRONG — severs the error chain
type Model struct { initErr string }
initErr = fmt.Sprintf("failed to get working directory: %s", err)  // ✗

// RIGHT — preserve the chain; .Error() only at the display boundary
type Model struct { initErr error }
initErr = fmt.Errorf("failed to get working directory: %w", err)   // ✓
```

- **No silent fallbacks.** Do not default or silently recover from invalid user-supplied data.
- **NEVER panic.** A `panic` crashes the process and loses unsaved content. Malformed input is inevitable. If you need to enforce an invariant, use a safe fallback (graceful degradation) or a test-only assertion (`//go:build testing`). Rendering errors are tolerable, data loss is not.

### 1.4 Data Persistence & Durability

These rules keep the user's words on the lowest rungs of the harm ladder (§0.1). They are implementation-agnostic law: however persistence is built, it MUST satisfy every rule below.

**1.4.1 Disk writes are atomic and durable — behind one shared helper.**
Never `os.WriteFile` / truncate-in-place over a user's file: a crash or power loss mid-write leaves a half-written or empty file (Catastrophic). Every write of user content to disk MUST:
1. Write to a temp file in the **same directory** as the target (same filesystem, so the rename is atomic).
2. `Sync()` the temp file — flush the bytes to durable storage.
3. `os.Rename` the temp over the target — an atomic replace; a reader sees either the old file or the new one, never a partial.
4. Where the platform supports it, `fsync` the parent directory so the rename itself survives a crash.

A save is not "done" — and the buffer is not "clean" — until the rename returns without error. This logic lives in **one** function that every writer calls (`pkg/atomicfile`).

**1.4.2 Never auto-persist a possibly-bad buffer onto the user's file.**
An automatic, debounced write that copies the live buffer straight onto the destination `.md` turns one bad in-memory edit into permanent on-disk loss. Durability for *unsaved* work MUST go to a separate recovery store (a journal / snapshot), never to the user's file. The destination file changes only on an explicit, intentional act — save (⌘S) and save-on-close. Background persistence protects work without ever overwriting the original with unreviewed content.

**1.4.3 Recovery state must itself survive the crash.**
At most a few seconds of work may ever be at risk (rung 2). Whatever backs crash recovery — journal, log, snapshot — MUST be durable. State that lives only in memory cannot survive the crash it exists to protect against. On open, reconstruct any unsaved changes from the recovery store and present them to the user as *editable* history they can keep or step back through; never silently write recovered content back over the file on disk.

**1.4.4 Guard every destructive transition.**
Closing a tab, quitting, switching away from, or overwriting a *dirty* buffer MUST NOT silently discard edits. Prompt (save / discard / cancel) or preserve — no code path drops unsaved changes without an explicit user choice. The confirmation state lives in the component that renders it (`pkg/ui/CLAUDE.md` §3.2), never scattered across a page.

**1.4.5 Round-trip byte fidelity — no silent normalization.**
What the user opened is what they get back, byte for byte, unless they edited it. Do NOT silently normalize line endings (CRLF/LF/CR), add or strip a trailing newline, transcode, or insert/remove a BOM. These "helpful" rewrites are silent degradation (rung 1): they corrupt diffs, pollute version-control history, and desync files shared with other tools. Preserve the original line-ending style and trailing-newline state across load → edit → save. Any normalization is an explicit, opt-in user action.

**1.4.6 Key history to the file, not its path.**
A document's recovery history belongs to the *file*, not the string naming it. Identify files by a stable identity (e.g. inode + device) rather than path, so renaming a closed file does not orphan its history and reusing a path for a different file does not graft on the wrong history. When a rename is detected, tell the user. Treat path as a fallback identifier only.

**1.4.7 Treat an externally-changed file as a hazard.**
A file may change on disk after you load it — another editor, a `git` operation, a sync client. Before overwriting, detect divergence from what you loaded (mtime / size / content hash) and refuse-or-prompt rather than clobber the newer version. Surface the conflict; never silently win.

### 1.5 Text & Strings

**`len()` is byte length, not character length.** When a string is used for display — terminal output, table cells, cursor positions, wrap calculations — use `utf8.RuneCountInString` instead of `len`.

```go
// WRONG — byte length, breaks on multi-byte unicode
width := len("café")        // 5, not 4
col := len(line[:cursor])   // wrong column if line has non-ASCII

// RIGHT — rune count = display width for CJK, emoji, accents, etc.
width := utf8.RuneCountInString("café")  // 4
col := utf8.RuneCountInString(line[:cursor])
```

This applies to every calculation that touches rendering: span distance, cell position, column index, wrap boundaries, table cell widths. **Assume all input strings may contain multi-byte characters.** If a `len()` call is on display-related code, it is almost certainly wrong.

### 1.6 Project Organization
- NEVER create `types`, `utils`, `helpers`, `common`, or `misc` packages.
- One primary type per file. A file named `editor.go` owns `type Model struct` for the editor and its immediate helpers.
- 500 LoC per file is a hard smell. If a component exceeds this, decompose it.

---

## 2.–7. TUI Architecture → `pkg/ui/CLAUDE.md`

Sections §2–§7 — **Component Architecture, Keybinding Architecture, Layout & Dimensions, the Elm Cycle, Context & Async Operations, and Page Transitions & Routing** — govern the Bubble Tea UI and live in **`pkg/ui/CLAUDE.md`**. Their section numbers are unchanged (stable: source code and ADRs cite them, e.g. §3.2, §6.3).

**If you are editing any file under `pkg/ui/` and have not already loaded `pkg/ui/CLAUDE.md`, read it now.** Subdirectory memory files load on demand — only when a file in that directory is read — and are not re-injected after a context compaction, so it may be absent even mid-task. Do not assume it loaded automatically.

---

## 8. Testing

> **Editor-specific QA:** See `qa-instructions.md` for the full testing architecture governing `pkg/editor/`, `pkg/command/`, and editor integration tests.

### 8.1 Mandatory Test Classes

When changing TUI behavior, add tests for the affected invariant class:

| Class | What to assert |
|-------|---------------|
| Render Purity | Call `View()` N times, assert identical output. No side effects. |
| Layout | Resize to small/large terminals. Assert no overlapping bounds. Assert children receive correct dimensions. |
| Scroll Stability | Set content while user has scrolled. Assert offset is preserved (or intentionally reset). |
| Async I/O | Send success and failure messages. Assert model state transitions correctly. Test missing files, permission errors. |
| Key Routing | Assert focused component receives keys. Assert unfocused component ignores keys. Assert no binding collisions. |

### 8.2 Test Patterns

- NEVER use `time.Sleep` for synchronization in tests.
- Test the `Update` → `Model` cycle directly. Feed messages, assert resulting model fields.
- For Cmd testing: execute the returned Cmd, assert the resulting Msg type and contents.

---

## 9. LLM-Specific Pitfalls (Data Safety & Go)

Mistakes LLMs commonly produce in this codebase; each is a hard violation. (UI-architecture pitfalls are in `pkg/ui/CLAUDE.md` §9.)

| # | Anti-Pattern | Why It's Wrong | Rule Violated |
|---|---|---|---|
| 1 | Creating `types`, `utils`, `helpers`, `common` packages | Catch-all packages with no cohesion | §1.6 |
| 2 | Using `len(str)` instead of `utf8.RuneCountInString(str)` for display-related calculations | Byte length ≠ character length; breaks CJK, emoji, accented chars | §1.5, `pkg/ui/CLAUDE.md` §4.5 |
| 3 | Returning or storing an error as `string` instead of `error` | Severs the error chain; use `%w` and keep the `error` type | §1.3 |
| 4 | Writing a user file with `os.WriteFile`/truncate-in-place instead of atomic temp→fsync→rename | A crash mid-write corrupts or empties the file | §0.1, §1.4.1 |
| 5 | Auto-saving the live buffer straight onto the user's `.md` on a debounce timer | One bad in-memory edit becomes permanent disk loss; unsaved work belongs in the recovery store | §1.4.2 |
| 6 | Applying an empty/whitespace async edit (dictation/paste/IME reset) as a destructive `ReplaceRange` | An upstream reset masquerades as a user deletion and wipes committed text | §1.3, §1.4 |
| 7 | Closing/quitting/overwriting a dirty buffer with no save/discard/cancel prompt | Silently loses unsaved work (rung 2) | §1.4.4 |
| 8 | Silently normalizing line endings / trailing newline / encoding / BOM on save | Silent degradation (rung 1): corrupts diffs and round-trip fidelity | §1.4.5 |
| 9 | Backing crash recovery with in-memory-only state | Evaporates on the very crash it exists to survive | §1.4.3 |
| 10 | Keying recovery/undo history to a path string instead of the file's stable identity | Rename orphans history; path reuse grafts on the wrong history | §1.4.6 |

---

## 10. Directory Structure

```
pkg/ui/                         Top-level router (app.go)
pkg/ui/components/              Reusable, isolated UI widgets
pkg/ui/components/editor/       Editor pane (viewport + breadcrumb + file content)
pkg/ui/components/filetree/     File list navigation
pkg/ui/components/opentabs/     Open files tab bar
pkg/ui/components/footer/       Status bar + chord confirmation + help
pkg/ui/components/breadcrumb/   Path breadcrumb (may be internal to editor)
pkg/ui/pages/                   Full-screen views composed of components
pkg/ui/pages/workspace/         Main editing workspace (3-pane layout)
pkg/ui/styles/                  Shared styling definitions (Styles struct)
pkg/ui/keymap/                  Shared keybindings (Bindings struct)
```

**Import rules:**
- Components MUST NOT import pages.
- Components MUST NOT import other components at the same level (no lateral deps). If two components need to communicate, they do so via messages routed through their parent page.
- Pages import components.
- A page MUST NOT import implementation-detail packages of its children. If a page imports `viewport` directly, that is a sign an `editor` component is missing — the viewport is an internal detail of the editor.
- `styles` and `keymap` are leaf packages — they import nothing from `pkg/ui/`.

---

## 11. Pre-Merge Checklist (Data Safety & Go)

Before completing any change, mechanically verify. (UI-architecture checks are in `pkg/ui/CLAUDE.md` §11.)

- [ ] Data integrity failures stop execution and surface explicit errors to the user.
- [ ] No silent error swallowing — every error is wrapped with context or explicitly annotated.
- [ ] No `error` downgraded to `string` — `.Error()` called only at the display/log boundary.
- [ ] Every write of user content to disk goes through the shared atomic, durable helper (temp → `fsync` → `rename`) — no raw `os.WriteFile`/truncate-in-place.
- [ ] No automatic/debounced write copies the live buffer onto the user's destination file; unsaved work is persisted only to the recovery store.
- [ ] Crash-recovery state is durable, not in-memory-only.
- [ ] Every destructive transition (close / quit / overwrite / switch on a dirty buffer) prompts or preserves — never a silent discard.
- [ ] No silent normalization of line endings, trailing newline, encoding, or BOM — load → save is byte-faithful unless the user edited.
- [ ] Async/external edits (dictation, paste, IME, watchers) are validated and clamped to the live buffer; an empty/whitespace upstream reset never applies as a destructive replace.
- [ ] Document recovery identity is keyed to the file's stable identity (inode+device), with path as fallback.
- [ ] No file exceeds 500 LoC.
- [ ] No `len()` used for display-related string length; `utf8.RuneCountInString` used instead.
