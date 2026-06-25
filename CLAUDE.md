# Rune — Engineering Standard & Code Convention

`rune` is a Bubble Tea (v2) TUI markdown editor in Go. This file is the binding engineering standard; MUST/NEVER are law.

---

## 0. Prime Directive — Protect the User's Words

Never corrupt and never lose what the user has written. When data safety conflicts with performance, elegance, or feature correctness, data safety wins.

### 0.1 The Harm Ladder

Rank every defect by the highest rung it can reach:

1. **Catastrophic — silent corruption.** Wrong or garbled bytes, mangled UTF-8, dropped or reordered content, a silent rewrite (line endings, trailing newline, BOM, encoding), a good file overwritten by a bad buffer. The user may not notice until the original is gone. Never ships.
2. **Severe — losing more than a few seconds of work.** A crash, failed save, or botched recovery that discards unsaved edits. At most a few seconds may ever be at risk.
3. **Tolerable — everything else.** Render glitch, wrong layout, dropped keypress, a clean crash that loses nothing. Always prefer a Tolerable failure — halt with a visible error, keep the buffer — over the rungs above. That halt is a surfaced error, never a `panic` (which takes the unsaved buffer with it).

---

## 1. Go Fundamentals

### 1.1 Value Semantics
- Use Go values over pointers. Only use pointers when a struct contains a `sync.Mutex` or owns a unique OS resource (file handle, socket).
- Models own their child models **by value**. NEVER store pointers to other models.
- Zero values MUST be meaningful. Prefer `type Result struct { Data T; Valid bool }` over `*T` with nil checks.
- Reset a *reusable* slice in place with `xs = xs[:0]`, never `xs = nil`. Slicing to zero keeps the backing array so the next fill reuses it; `nil` throws the array away, forcing a re-allocation and leaving the old one for the garbage collector. Reserve `xs = nil` for when you are intentionally *releasing* memory you will not refill (e.g. dropping a large decoded buffer) — not for clearing a buffer you are about to fill again.

### 1.2 Concurrency
- Share memory by communicating. Data crossing goroutine boundaries MUST be passed by copy or be strictly immutable.
- NEVER access a Model field from inside a `tea.Cmd` closure. Copy the needed value into a local variable first.

### 1.3 Error Handling
- **Fail fast on data risk.** If there is ANY suspicion that user data may be corrupted or lost, STOP operations immediately and surface a hard error to the user.
- **Validate before you persist a destructive change.** An edit that collapses a non-empty region to empty, deletes a large fraction of the buffer, or shrinks a document far below what was loaded is *suspect until proven intentional*. Edits arriving from asynchronous or external sources (dictation, paste, IME composition, network, file watchers) are especially suspect: an upstream *reset* (e.g. an empty interim result) MUST NOT be applied as a user *deletion*. Drop or clamp such an edit; never let it reach the buffer — and therefore never let it reach disk.
- **Clamp every edit range; never silently drop the failure.** All edits funnel through `buffer.ApplyEdits` / `textedit.ReplaceRange`; offsets are **byte** offsets (§1.5). Clamp `[start, end]` to the live byte length and keep UTF-8 boundaries intact. A range computed against an old revision (dictation, paste, async, IME) is a corruption vector — clamp it, or surface/log the rejection. Do NOT `if err != nil { return m }` it away: a silent drop loses a paste/dictation edit and hides offset bugs.
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

Implementation-agnostic law: however persistence is built, it MUST satisfy every rule below. Read this section before touching any code that writes to disk, mutates the buffer, or closes a document.

**1.4.1 Disk writes are atomic and durable — behind one shared helper.** Never `os.WriteFile`/truncate-in-place over a user's file (a crash mid-write leaves a half-written or empty file). Every write of user content MUST: (1) write a temp file in the **same directory** as the target; (2) `Sync()` it; (3) `os.Rename` it over the target — an atomic replace; (4) where supported, `fsync` the parent directory. The buffer is not "clean" until the rename returns without error. One function, every writer (`pkg/atomicfile`).

**1.4.2 Never auto-persist a possibly-bad buffer onto the user's file.** A debounced write that copies the live buffer onto the destination `.md` turns one bad in-memory edit into permanent loss. Unsaved work goes to a separate recovery store (journal/snapshot), never the user's file. The destination changes only on an explicit act — save (⌘S) and save-on-close.

**1.4.3 Recovery state must itself survive the crash.** Whatever backs crash recovery (journal/snapshot) MUST be durable — in-memory-only state evaporates in the crash it exists to survive (rung 2). On open, reconstruct unsaved changes from the store and present them as *editable* history the user can keep or step back through; never silently write recovered content back over the file.

**1.4.4 Guard every destructive transition.** Closing, quitting, switching away from, or overwriting a *dirty* buffer MUST prompt (save/discard/cancel) or preserve — no path drops unsaved changes without an explicit user choice. The confirmation state lives in the component that renders it (`pkg/ui/CLAUDE.md` §3.2).

**1.4.5 Byte-faithful round-trip — never normalize.** Load → edit → save is byte-identical except where the user edited. NEVER silently normalize line endings (CRLF/LF/CR), add/strip a trailing newline, transcode, or insert/remove a BOM — silent degradation (rung 1) that corrupts diffs and VCS history. The editor writes bytes verbatim today; keep it that way. Normalization is an explicit, opt-in user action.

**1.4.6 Key history to the file, not its path.** Identify files by stable identity (inode + device), not the path string, so renaming a closed file does not orphan its history and reusing a path does not graft on the wrong history. On a detected rename, tell the user. Path is a fallback identifier only.

**1.4.7 Treat an externally-changed file as a hazard.** A file may change on disk after load (another editor, `git`, a sync client). Before overwriting, detect divergence from what you loaded (size/mtime, or a content hash) and refuse-or-prompt rather than clobber the newer version. Surface the conflict; never silently win.

**1.4.8 Dirty is derived — recompute it on every transition.** "Dirty" is not a cached flag; it is the durable journal position vs the last-saved position. On every transition — open, tab switch, LRU eviction, close, quit — reload the position from the store and recompute. Never trust a flag cached from when the tab was last active; never let a transition silently drop or mis-mark a dirty buffer.

**1.4.9 One filesystem — everything goes through `vfs.FS`.** Every filesystem operation the running editor performs — read, write, stat, readdir, mkdir, rename, existence check — MUST go through the injected `vfs.FS` (`pkg/vfs`), never a direct `os.*`/`ioutil`/`filepath.Walk` call. A component that needs the filesystem receives `vfs.FS` by dependency injection (as the workspace's `fsys()`, docstate's `UseFS`, and the editor's `SetFS` do) and defaults a `nil` shim to `vfs.Disk{}`; it never reaches for `os` itself. This is not stylistic: a stray `os.Stat`/`os.ReadFile` silently consults real disk instead of the FS the rest of the app is using, so in-memory tests and the session fuzzer diverge from production (e.g. cross-links resolving as "missing" because the resolver statted disk while the workspace served `vfs.Mem`) — and any future sandbox/remote backend is bypassed. There are exactly **two sanctioned boundaries** where real `os.*` lives: `pkg/vfs` (the `FS` implementations — `Disk` wraps `os`, `Mem` is in-memory) and `pkg/atomicfile` (the durable temp→fsync→rename primitive `vfs.Disk.WriteFile` delegates to). Three narrow, **documented** exceptions sit OUTSIDE document I/O: (a) launch bootstrap that must run before any FS exists — CWD resolution (`os.Getwd`), `-w` argument validation, and the SQLite recovery-DB path under the user data dir (SQLite owns a real file handle and cannot live in `vfs.Mem`); (b) the fsnotify directory watcher (the OS watch API has no in-memory analogue — it is `//go:build fuzzing`-stubbed and tests inject watch *messages*); (c) test/fuzz tooling (`*_test.go`, `internal/fuzz/**`, golden-file helpers). Anything else doing direct `os.*` FS I/O in `pkg/ui/**` or other runtime code is a violation.

*Implementation map (the laws above stay implementation-agnostic; this is today's wiring): atomic write → `pkg/atomicfile.Write`; recovery store → `pkg/docstate` (`AppendEdit` / `CreateSnapshot` / `RecoverDocument`); dirty state → `workspace` journal positions (`DocJournalPos` / `RecordSaved`); filesystem → `pkg/vfs.FS` injected via `workspace.WithFS` → `markdownedit.SetFS` / `docstate.UseFS`, `nil`-defaulting to `vfs.Disk{}`.*

### 1.5 Two Coordinate Systems — Bytes vs. Runes

`rune` has two length spaces, and mixing them corrupts either the buffer or the layout. Always know which one a number lives in.

- **Storage / edit / cursor offsets are BYTES.** `cursor.Position`, `buffer.Replace(start, end, …)`, and every `ReplaceRange` offset are byte offsets, validated against `len(content)`. Compute them with `len(...)`; never split a UTF-8 rune.
- **Display width is RUNES.** Column, cursor column, wrap boundary, table-cell width, span distance — use `utf8.RuneCountInString`, never `len`.

```go
end := start + len(insertedText)                  // edit offset — BYTES ✓ (RuneCount here misplaces the cursor)
col := utf8.RuneCountInString(line[:cursorByte])  // display column — RUNES ✓ (len here breaks on CJK/emoji/accents)
```

If the number indexes into the buffer, it is bytes; if it positions something on screen, it is runes. A `len()` in rendering code is almost certainly a bug; a `utf8.RuneCountInString` used as a buffer or cursor offset is almost certainly a corruption.

### 1.6 Project Organization
- NEVER create `types`, `utils`, `helpers`, `common`, or `misc` packages.
- One primary type per file. A file named `editor.go` owns `type Model struct` for the editor and its immediate helpers.
- 500 LoC per file is a hard smell. If a component exceeds this, decompose it.

### 1.7 One Value, One Meaning — No Magic Sentinels

A value carries exactly one meaning. NEVER overload a variable with a magic sentinel so a single field encodes both *"is there a value?"* and, if so, *"what is it?"* — e.g. `length == -1` for "invalid", `idx == -1` for "nothing selected", `name == ""` for "unset". The reader must now remember the sentinel at every site that touches the value, and one missed check silently treats the sentinel (`-1`, `""`, `0`) as real data. That is a bug factory — and when the value drives an edit offset or length, a corruption vector (§1.3).

**In memory:** represent presence/validity *out of band*. Use a separate boolean or an explicit option type — the `type Result struct { Data T; Valid bool }` pattern from §1.1, or the standard library's `sql.NullInt64{ Int64, Valid }`. Check `Valid` before reading the value; the data field on its own carries no "absent" meaning.

**In the database:** the absence of a value is `NULL` — never `-1`, never the strings `"undefined"` / `"none"` / `""`, never `0`. `NULL` is the one representation the schema, the query planner, and every reader already agree means "absent"; a magic value collides with legitimate data and forces every consumer to know the convention. `rune`'s store (`pkg/docstate`) already does this correctly: nullable columns such as `current_seq` / `saved_seq` are `NULL` when absent and read back through `sql.NullInt64` + `.Valid`. Hold new schema and queries to that standard.

```go
// WRONG — -1 means "no clean point"; the meaning lives only in the reader's head
cleanPos := -1                    // sentinel: "never saved"
dirty := journalPos != cleanPos   // ✗ skip the guard and -1 acts as a real position

// RIGHT — validity is a separate fact from the value
var clean sql.NullInt64           // .Valid answers "is there a clean point?"; .Int64 is the point
dirty := !clean.Valid || journalPos != clean.Int64
```

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

Each is a hard violation; the cited section is the rule. (UI-architecture pitfalls are in `pkg/ui/CLAUDE.md` §9.)

| # | Anti-Pattern | Rule |
|---|---|---|
| 1 | A `types` / `utils` / `helpers` / `common` catch-all package | §1.6 |
| 2 | Wrong coordinate system — `len` as a display width, or a rune count as a buffer/cursor offset | §1.5 |
| 3 | Storing or returning an `error` as a `string` (severs the `%w` chain) | §1.3 |
| 4 | `os.WriteFile`/truncate-in-place on a user file instead of atomic temp→fsync→rename | §1.4.1 |
| 5 | Debounced auto-save of the live buffer onto the user's `.md` | §1.4.2 |
| 6 | Applying an empty/whitespace async reset (dictation/paste/IME) as a destructive `ReplaceRange` | §1.3 |
| 7 | Closing/quitting/overwriting a dirty buffer with no save/discard/cancel prompt | §1.4.4 |
| 8 | Silently normalizing line endings / trailing newline / BOM / encoding | §1.4.5 |
| 9 | Crash-recovery state that lives only in memory | §1.4.3 |
| 10 | Keying recovery/undo history to a path string, not inode+device | §1.4.6 |
| 11 | Resetting a reusable slice with `= nil` instead of `[:0]` | §1.1 |
| 12 | A magic sentinel (`-1` / `""` / `0`) overloading a variable to mean "invalid/absent" | §1.7 |
| 13 | A magic value for an absent DB value instead of `NULL` | §1.7 |
| 14 | Caching a dirty flag across a transition instead of recomputing it from the store | §1.4.8 |
| 15 | Direct `os.*`/`ioutil`/`filepath.Walk` FS access in runtime code instead of the injected `vfs.FS` | §1.4.9 |

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

- [ ] Data-integrity failures stop and surface an explicit error.
- [ ] No silent error swallowing; no `error` downgraded to `string` (`.Error()` only at the display/log boundary).
- [ ] All user-content disk writes go through `atomicfile.Write` (atomic temp→`fsync`→`rename`) — no raw `os.WriteFile`/truncate.
- [ ] No automatic/debounced write copies the live buffer onto the destination file; unsaved work → recovery store only.
- [ ] Crash-recovery state is durable, not in-memory-only.
- [ ] Every destructive transition (close/quit/overwrite/switch on a dirty buffer) prompts or preserves — never a silent discard.
- [ ] Dirty state is recomputed from the store on every transition (open/switch/evict/close) — never a cached flag. (§1.4.8)
- [ ] No silent normalization of line endings, trailing newline, BOM, or encoding.
- [ ] Async/external edits are clamped to the live buffer; an empty/whitespace reset never applies as a destructive replace, and the clamp rejection is surfaced, not swallowed.
- [ ] Recovery identity is keyed to inode+device, with path as fallback.
- [ ] Edit/cursor offsets are computed in **bytes** (`len`); display widths in **runes** (`utf8.RuneCountInString`) — never mixed. (§1.5)
- [ ] No variable carries a magic sentinel (`-1`/`""`/`0`) for "invalid/absent"; DB absence is `NULL` (read via `sql.NullXxx` + `.Valid`). (§1.7)
- [ ] Reusable slices are reset with `[:0]`, not `nil` (`nil` only to release memory you won't refill).
- [ ] All filesystem access in runtime code goes through the injected `vfs.FS` — no direct `os.*`/`ioutil`/`filepath.Walk`, except the `pkg/vfs` + `pkg/atomicfile` boundaries and the documented bootstrap/watcher/test exceptions. (§1.4.9)
- [ ] No file exceeds 500 LoC.
