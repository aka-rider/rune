# Rune — Engineering Standard & Code Convention

`rune` is a Bubble Tea (v2) TUI markdown editor in Go. This is the binding engineering standard — follow every rule exactly.

## Read first — the unbreakables (rung-1; full rule at the cited §)
- Write the user's bytes verbatim — no normalized line endings / trailing newline / BOM / encoding. §1.4.5
- Write user content only through `atomicfile.Write` (atomic temp→fsync→rename). §1.4.1
- Clamp every edit range to the live byte length; an empty async reset is not a deletion. §1.3
- Edit/cursor offsets are BYTES (`len`); display widths are RUNES (`utf8.RuneCountInString`). §1.5
- Halt with a surfaced error, never `panic` — a panic loses the unsaved buffer. §1.3
- Reach the filesystem only through the injected `vfs.FS`. §1.4.9

---

## 0. Prime Directive — Protect the User's Words

Never corrupt and never lose what the user wrote. When data safety conflicts with performance, elegance, or features — data safety wins.

### 0.1 The Harm Ladder

Rank every defect by the highest rung it can reach:

1. **Catastrophic — silent corruption.** Wrong/garbled bytes, mangled UTF-8, dropped or reordered content, a silent rewrite (line endings, trailing newline, BOM, encoding), a good file overwritten by a bad buffer. The user may not notice until the original is gone. Never ships.
2. **Severe — losing more than a few seconds of work.** A crash, failed save, or botched recovery that discards unsaved edits. At most a few seconds may ever be at risk.
3. **Tolerable — everything else.** Render glitch, wrong layout, dropped keypress, a clean crash that loses nothing.

Always prefer a Tolerable failure — halt with a visible error and keep the buffer — over a higher rung. That halt is a surfaced error, never a `panic` (which takes the unsaved buffer with it).

---

## 1. Go Fundamentals

### 1.1 Value Semantics
- Prefer values over pointers. Use a pointer only when a struct holds a `sync.Mutex` or owns a unique OS resource (file handle, socket).
- Own child models **by value**; never store a pointer to another model.
- Make zero values meaningful. Prefer `type Result struct { Data T; Valid bool }` over `*T` + nil checks.
- Reset a *reusable* slice with `xs = xs[:0]` — it keeps the backing array for the next fill. Use `xs = nil` only to *release* memory you won't refill (e.g. dropping a large decoded buffer); `nil` discards the array and forces a re-allocation.

### 1.2 Concurrency
Cross goroutine boundaries by copy or immutable value only. In a `tea.Cmd` closure, read only locals captured before it:
```go
idx := m.opentabs.Cursor()                  // ✓ capture, then close over
return func() tea.Msg { return PinMsg{idx} }
// func()…{ PinMsg{m.opentabs.Cursor()} }    ✗ reads m later, when it runs → race
```

### 1.3 Error Handling
- **Fail fast on data risk.** On any suspicion user data is lost or corrupted, stop and surface a hard error.
- **Validate before persisting a destructive change.** Treat as suspect-until-proven any edit that empties a non-empty region, deletes a large fraction, or shrinks the doc far below load size — especially from async/external sources (dictation, paste, IME, network, file watcher). An empty interim reset is not a user deletion: clamp or drop it before it reaches the buffer, so it never reaches disk.
- **Clamp every edit range.** All edits funnel through `buffer.ApplyEdits` / `textedit.ReplaceRange`; offsets are BYTES (§1.5). Clamp `[start, end]` to live byte length on UTF-8 boundaries, and surface or log a rejected clamp:
```go
end = min(end, len(content))   // ✓ clamp to live byte length; a stale range is a corruption vector
// if err != nil { return m }   ✗ silent drop loses a paste/dictation edit and hides offset bugs
```
- **Handle the error every call returns.** To ignore one, annotate it: `_ = f()  // fire-and-forget: <reason>`.
- **Wrap with context and `%w`** (preserves `errors.Is`/`As`): `fmt.Errorf("load dir %q: %w", dir, err)`.
- **Keep errors as `error`, not `string`;** call `.Error()` only at the display boundary:
```go
initErr error                              // ✓ keeps the %w chain
// initErr = fmt.Sprintf("...: %s", err)   ✗ severs the chain; errors.Is/As stop working
```
- **No silent fallback** — don't default or recover from invalid user input.
- **Never `panic`** — it crashes the process and loses unsaved content. Enforce an invariant with a safe fallback (graceful degradation) or a test-only assert (`//go:build testing`). Rendering errors are tolerable; data loss is not.

### 1.4 Data Persistence & Durability

These laws are implementation-agnostic — however persistence is built, it satisfies every one. Read this before touching code that writes to disk, mutates the buffer, or closes a document.

**1.4.1 Atomic, durable writes — one shared helper.** Write every piece of user content through `atomicfile.Write`: temp file in the target's directory → `Sync()` → `os.Rename` over the target (atomic replace) → `fsync` the parent dir where supported. The buffer is clean only after the rename returns. Never `os.WriteFile`/truncate a user file in place — a mid-write crash leaves it half-written or empty.

**1.4.2 Persist unsaved work to the recovery store, never the user's file.** A debounced write that copies the live buffer onto the destination `.md` turns one bad in-memory edit into permanent loss. Unsaved work goes to the recovery store (journal/snapshot); the destination changes only on an explicit act — save (⌘S) or save-on-close.

**1.4.3 Make recovery state durable.** Whatever backs crash recovery survives the crash — in-memory-only state evaporates in the crash it exists to survive (rung 2). On open, reconstruct unsaved changes from the store as *editable* history the user can keep or step back through; never silently write recovered content over the file.

**1.4.4 Guard every destructive transition.** Closing, quitting, switching away from, or overwriting a *dirty* buffer prompts (save/discard/cancel) or preserves — no path drops unsaved changes without an explicit user choice. The confirmation state lives in the component that renders it (`pkg/ui/CLAUDE.md` §3.2).

**1.4.5 Byte-faithful round-trip.** Load → edit → save is byte-identical except where the user edited. Never silently normalize line endings (CRLF/LF/CR), add/strip a trailing newline, transcode, or insert/remove a BOM — silent degradation (rung 1) that corrupts diffs and VCS history. Normalization is an explicit, opt-in user action.

**1.4.6 Key history to the file's identity, not its path.** Identify a file by stable identity (inode + device), so renaming a closed file keeps its history and reusing a path doesn't graft on the wrong history. Tell the user on a detected rename. Path is a fallback identifier only.

**1.4.7 Treat an externally-changed file as a hazard.** A file may change on disk after load (another editor, `git`, a sync client). Before overwriting, detect divergence from what you loaded (size/mtime or a content hash) and refuse-or-prompt; surface the conflict rather than clobber the newer version.

**1.4.8 Derive dirty on every transition.** "Dirty" is the durable journal position vs the last-saved position, not a cached flag. On every transition — open, tab switch, LRU eviction, close, quit — reload the position from the store and recompute. Never trust a flag cached from when the tab was last active, and never let a transition silently drop or mis-mark a dirty buffer.

**1.4.9 Reach the filesystem only through `vfs.FS`.** Every filesystem operation the running editor performs — read, write, stat, readdir, mkdir, rename, existence check — goes through the injected `vfs.FS` (`pkg/vfs`), not a direct `os.*`/`ioutil`/`filepath.Walk`. A component that needs the filesystem receives `vfs.FS` by injection (workspace's `fsys()`, `docstate.UseFS`, the editor's `SetFS`) and defaults a `nil` shim to `vfs.Disk{}`. This is load-bearing: a stray `os.Stat`/`os.ReadFile` consults real disk instead of the FS the rest of the app uses, so in-memory tests and the session fuzzer diverge from production (e.g. cross-links resolving as "missing" because the resolver statted disk while the workspace served `vfs.Mem`), and any future sandbox/remote backend is bypassed. Real `os.*` lives at exactly two sanctioned boundaries: `pkg/vfs` (the `FS` implementations — `Disk` wraps `os`, `Mem` is in-memory) and `pkg/atomicfile` (the temp→fsync→rename primitive `vfs.Disk.WriteFile` delegates to). Three documented exceptions sit outside document I/O: (a) launch bootstrap before any FS exists — `os.Getwd`, `-w` argument validation, the SQLite recovery-DB path under the user data dir (SQLite owns a real file handle, can't live in `vfs.Mem`); (b) the fsnotify directory watcher (no in-memory analogue — `//go:build fuzzing`-stubbed; tests inject watch *messages*); (c) test/fuzz tooling (`*_test.go`, `internal/fuzz/**`, golden helpers). Any other direct `os.*` FS I/O in `pkg/ui/**` or runtime code is a violation.

*Today's wiring (the laws stay implementation-agnostic): atomic write → `atomicfile.Write`; recovery store → `docstate` (`AppendEdit` / `CreateSnapshot` / `RecoverDocument`); dirty state → `docstate` journal positions (`CurrentSeq` / `MarkSavedAt` / `IsDirty`); filesystem → `vfs.FS` injected via `workspace.WithFS` → `markdownedit.SetFS` / `docstate.UseFS`, `nil`-defaulting to `vfs.Disk{}`.*

### 1.5 Two Coordinate Systems — Bytes vs. Runes

Two length spaces; mixing them corrupts either the buffer or the layout. Always know which a number lives in.

- **Storage / edit / cursor offsets are BYTES.** `cursor.Position`, `buffer.Replace(start, end, …)`, every `ReplaceRange` offset — byte offsets validated against `len(content)`. Compute with `len(...)`; never split a UTF-8 rune.
- **Display width is RUNES.** Column, cursor column, wrap boundary, table-cell width, span distance — use `utf8.RuneCountInString`, never `len`.

```go
end := start + len(insertedText)                  // edit offset — BYTES ✓ (RuneCount misplaces the cursor)
col := utf8.RuneCountInString(line[:cursorByte])  // display column — RUNES ✓ (len breaks on CJK/emoji/accents)
```

If the number indexes into the buffer it's bytes; if it positions something on screen it's runes. A `len()` in rendering code is almost certainly a bug; a `utf8.RuneCountInString` used as a buffer/cursor offset is almost certainly a corruption.

### 1.6 Project Organization
- Never create a `types` / `utils` / `helpers` / `common` / `misc` package.
- One primary type per file — `editor.go` owns the editor's `type Model struct` and its immediate helpers.
- 500 LoC per file is a hard smell; decompose past it.

### 1.7 One Value, One Meaning — No Magic Sentinels

A value carries exactly one meaning. Don't overload a variable with a sentinel that makes one field encode both *"is there a value?"* and *"what is it?"* — `length == -1` for "invalid", `idx == -1` for "nothing selected", `name == ""` for "unset". Every reader must then remember the sentinel, and one missed check treats `-1`/`""`/`0` as real data — a bug factory, and a corruption vector when the value drives an edit offset or length (§1.3).

**In memory:** carry presence/validity out of band — a separate boolean or an option type (`type Result struct { Data T; Valid bool }` from §1.1, or `sql.NullInt64{ Int64, Valid }`). Check `Valid` before reading the value.

**In the database:** an absent value is `NULL` — not `-1`, not `"undefined"`/`"none"`/`""`, not `0`. `NULL` is the one representation schema, planner, and every reader already agree means "absent". `docstate` already does this — nullable columns like `current_seq` / `saved_seq` are `NULL` when absent and read back through `sql.NullInt64` + `.Valid`. Hold new schema to that standard.

```go
// ✗ -1 means "no clean point" — the meaning lives only in the reader's head
cleanPos := -1
dirty := journalPos != cleanPos   // skip the guard and -1 acts as a real position

// ✓ validity is a separate fact from the value
var clean sql.NullInt64           // .Valid answers "is there a clean point?"; .Int64 is the point
dirty := !clean.Valid || journalPos != clean.Int64
```

---

## 2.–7. TUI Architecture → `pkg/ui/CLAUDE.md`

Sections §2–§7 — **Component Architecture, Keybinding Architecture, Layout & Dimensions, the Elm Cycle, Context & Async Operations, Page Transitions & Routing** — govern the Bubble Tea UI and live in **`pkg/ui/CLAUDE.md`**. Their numbers are stable (source code and ADRs cite them, e.g. §3.2, §6.3).

**Editing any file under `pkg/ui/`? Load `pkg/ui/CLAUDE.md` now.** Subdirectory memory loads on demand and is not re-injected after a context compaction, so it may be absent mid-task — don't assume it loaded.

---

## 8. Testing

### 8.1 Mandatory Test Classes

Changing TUI behavior? Add tests for the affected invariant class:

| Class | What to assert |
|-------|---------------|
| Render Purity | Call `View()` N times → identical output, no side effects. |
| Layout | Resize small/large → no overlapping bounds; children get correct dimensions. |
| Scroll Stability | Set content while scrolled → offset preserved (or intentionally reset). |
| Async I/O | Send success and failure msgs → correct state transitions; test missing files, permission errors. |
| Key Routing | Focused component gets keys; unfocused ignores; no binding collisions. |

### 8.2 Test Patterns
- Synchronize with a deterministic clock, never `time.Sleep`.
- Test the `Update` → `Model` cycle directly: feed messages, assert resulting fields.
- For a Cmd: execute it, assert the resulting `Msg` type and contents.

---

## 9. LLM-Specific Pitfalls (Data Safety & Go)

Do the left — it's the fix for the middle. (UI pitfalls: `pkg/ui/CLAUDE.md` §9.)

| Do this | Instead of | § |
|---|---|---|
| Name a package for its domain | a `types`/`utils`/`helpers`/`common` catch-all | §1.6 |
| `len` for buffer/cursor offsets, `utf8.RuneCountInString` for display width | mixing the two coordinate systems | §1.5 |
| Store/return errors as `error`, chained with `%w` | downgrading an `error` to a `string` | §1.3 |
| Write user files via `atomicfile.Write` | `os.WriteFile`/truncate-in-place | §1.4.1 |
| Send unsaved work to the recovery store | debounced auto-save of the live buffer onto the `.md` | §1.4.2 |
| Clamp/drop an empty async reset (dictation/paste/IME) | applying it as a destructive `ReplaceRange` | §1.3 |
| Prompt save/discard/cancel on a dirty transition | closing/quitting/overwriting silently | §1.4.4 |
| Write bytes verbatim | normalizing line endings / newline / BOM / encoding | §1.4.5 |
| Make recovery state durable | crash-recovery state that lives only in memory | §1.4.3 |
| Key history to inode+device | keying it to a path string | §1.4.6 |
| Reset a reusable slice with `[:0]` | `= nil` (forces re-allocation) | §1.1 |
| Carry validity out of band (`Valid` bool / option type) | a `-1`/`""`/`0` sentinel for "invalid/absent" | §1.7 |
| Use `NULL` for an absent DB value | a magic value (`-1`/`""`) in the column | §1.7 |
| Recompute dirty from the store each transition | caching a dirty flag across transitions | §1.4.8 |
| Reach the FS through the injected `vfs.FS` | direct `os.*`/`ioutil`/`filepath.Walk` in runtime code | §1.4.9 |

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
- Components import neither pages nor sibling components at the same level. Two components that must talk do so via messages routed through their parent page.
- Pages import components — but not a child's implementation-detail packages. A page importing `viewport` directly signals a missing `editor` component (the viewport is the editor's internal detail).
- `styles` and `keymap` are leaf packages — they import nothing from `pkg/ui/`.

---

## 11. Pre-Merge Checklist (Data Safety & Go)

Mechanically verify before completing a change. (UI checks: `pkg/ui/CLAUDE.md` §11.)

- [ ] Data-integrity failures stop and surface an explicit error.
- [ ] Every call's error is handled or annotated `// fire-and-forget: …`; errors stay `error` (`.Error()` only at the display/log edge).
- [ ] User-content writes go through `atomicfile.Write` (atomic temp→`fsync`→`rename`).
- [ ] No automatic/debounced write copies the live buffer onto the destination; unsaved work → recovery store.
- [ ] Crash-recovery state is durable, not in-memory-only.
- [ ] Every destructive transition (close/quit/overwrite/switch on a dirty buffer) prompts or preserves.
- [ ] Dirty is recomputed from the store on every transition (open/switch/evict/close). (§1.4.8)
- [ ] Bytes stay verbatim — no normalized line endings, trailing newline, BOM, or encoding.
- [ ] Async/external edits are clamped to the live buffer; an empty/whitespace reset never applies as a destructive replace, and the clamp rejection is surfaced.
- [ ] Recovery identity is keyed to inode+device, with path as fallback.
- [ ] Edit/cursor offsets are computed in **bytes** (`len`); display widths in **runes** (`utf8.RuneCountInString`). (§1.5)
- [ ] No variable carries a magic sentinel (`-1`/`""`/`0`) for "invalid/absent"; DB absence is `NULL` via `sql.NullXxx` + `.Valid`. (§1.7)
- [ ] Reusable slices are reset with `[:0]`, not `nil`.
- [ ] All runtime filesystem access goes through the injected `vfs.FS` — no direct `os.*`/`ioutil`/`filepath.Walk` outside the `vfs`/`atomicfile` boundaries and documented bootstrap/watcher/test exceptions. (§1.4.9)
- [ ] No file exceeds 500 LoC.
