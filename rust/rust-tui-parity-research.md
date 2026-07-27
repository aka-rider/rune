# Rust TUI Parity Research — Explorer, Open Tabs, Footer

## Executive Summary

The Rust rune rewrite has a fully functional single-pane editor with an Elm-style runtime (Msg/Cmd/Effects), keymap resolver, clipboard, save/quit-confirm, and a minimal status line. The TUI framework is **ratatui 0.30.2** with the **termina** backend (ratatui-termina 0.1.0). Three critical UI components are missing to achieve 99.9% TUI parity with Go rune:

1. **Explorer (filetree)** -- flat directory listing with cursor navigation, file/dir selection, trash, fuzzy letter search, and mouse support.
2. **Open Tabs** -- tab bar with dirty indicators, pinning, eviction candidate logic, tab switching, and mouse support.
3. **Footer** -- full-featured bottom bar with cursor position, error/status messages with auto-dismiss, dictation indicator, data-loss guards (dirty/merge/deleted/raced/degraded/trash), chord quit confirmation, merge resolver hints, and default keybinding hints.

Additionally, the Rust codebase lacks: a multi-pane workspace layout, a styles/token system equivalent to Go's lipgloss-based `styles.Styles`, a shared list navigation abstraction, and workspace-level focus routing between panes.

---

## What the `sqlite` Session Already Delivered

The `sqlite` session produced two artifacts in the rust project:

- **`DATABASE MODEL.md`** -- A detailed architecture document for multiprocess SQLite. It recommends `rusqlite` with the `bundled` feature, `BEGIN IMMEDIATE` write discipline, `PRAGMA data_version` polling for cross-process notification, and WAL mode pragmas. It concluded that no existing Rust crate provides a robust multiprocess ACID SQLite policy layer, and recommended a ~400-line internal module. This document covers the *data layer* (WP2 in the roadmap) -- nothing in it addresses UI components.

- No additional plan, spec, or findings documents related to UI components were found in the sessions directory or the rust project. The `sqlite` session's scope was exclusively the database/storage layer.

---

## Current Rust Codebase Audit

### Crates

| Crate | Purpose | Status |
|-------|---------|--------|
| `rune-cli` | Binary entry point | Done -- bootstraps App, calls `runtime::run` |
| `rune-core` | Buffer, Cursor, Undo Journal, VFS (Disk/Mem) | Done |
| `rune-md` | Markdown parsing (comrak), syntax snapshot, wrap, cell emit | Done |
| `rune-tui` | Elm runtime, terminal, keymap, editor UI, render, status | Partial -- single-pane editor only |
| `rune-fuzz` | Property-based session fuzzer | Done |

### What Exists in `rune-tui`

| Module | File | What It Does |
|--------|------|--------------|
| `app` | `src/app.rs` | Elm model: `App` struct holding `Editor`, file path, VFS, save state, quit-confirm, status message |
| `editor` | `src/editor.rs` | Buffer + cursors + DocMachine + Viewport + Journal; `sync()` pipeline |
| `render` | `src/render.rs` | Cell model, `segment_cells`, `build_rows`, `blit`, `style_for(StyleId)` placeholder palette, `draw()` splits frame into editor + status |
| `status` | `src/status.rs` | ONE-row status line: file name, dirty dot (`\u{2022}`), status message, quit hint. Style: `fg(Black).bg(Gray)` |
| `runtime` | `src/runtime.rs` | Three-thread main loop: main (recv/draw), input reader, one thread per Cmd. `Msg`, `Cmd`, `Effects` |
| `term` | `src/term.rs` | `Guard` -- raw mode, alt screen, Kitty keyboard flags, bracketed paste, cursor hidden |
| `keymap` | `src/keymap.rs` | `KeyCode`, `Mods`, `KeyInput`, `Command` enum, `resolve()` table. Covers: navigation, selection, delete, indent, clipboard, undo/redo, save, quit-confirm |
| `clipboard` | `src/clipboard.rs` | OSC 52 copy, `pbpaste_cmd` paste read |
| `commands/edit` | `src/commands/edit.rs` | Insert, newline, delete, indent, outdent, undo, redo |
| `commands/nav` | `src/commands/nav.rs` | Character/word/line/page motion, select all, escape |
| `commands/clipboard` | `src/commands/clipboard.rs` | Copy, cut, paste |

### What is Missing

| Component | Go Source | Rust Equivalent |
|-----------|-----------|-----------------|
| Explorer (filetree) | `pkg/ui/components/filetree/` | **None** |
| Open Tabs | `pkg/ui/components/opentabs/` | **None** |
| Footer (full) | `pkg/ui/components/footer/` | `status.rs` (minimal subset -- no cursor pos, no guards, no dictation, no merge hints, no keybinding hints) |
| Multi-pane layout | `workspace_view.go:paneGeometry()` | **None** -- `render::draw()` only splits editor/status |
| Styles system | `pkg/ui/styles/styles.go` | **None** -- `render::style_for()` is a flat StyleId->Style map with no Palette, no derived styles |
| List navigation | `pkg/ui/listnav/listnav.go` | **None** |
| Workspace focus routing | `workspace_update_keys.go` | **None** -- `app.rs` has no concept of pane focus |
| Breadcrumb | `pkg/ui/components/breadcrumb/` | **None** |
| Title bar | `pkg/ui/components/title/` | **None** |
| Search bar | `pkg/ui/components/search/` | **None** |
| Chat pane | `pkg/ui/components/chat/` | **None** |

---

## Component Specifications

### 1. Explorer (Filetree)

**Go source:** `/Users/xiii/Developer/rune/pkg/ui/components/filetree/`

#### Public API

```go
type Model struct { /* entries, nav, root, width, height, offsetX, offsetY, focused, keys, styles, searchQuery, lastLetterAt */ }

func New(keys keymap.Bindings, st styles.Styles) Model
func (m Model) SetSize(w, h int) Model
func (m Model) SetOffset(x, y int) Model
func (m Model) SetFocused(f bool) Model
func (m Model) Cursor() int
func (m Model) Len() int
func (m Model) Focused() bool
func (m Model) Height() int
func (m Model) Root() string
func (m Model) RemoveEntry(path string) Model  // optimistic delete
func (m Model) Init() tea.Cmd
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)
func (m Model) View() string
```

#### Messages

```go
type FileSelectedMsg struct{ Path string }   // user pressed Enter on a file
type DirSelectedMsg struct{ Path string }     // user pressed Enter on a dir
type FileDeleteRequestedMsg struct{ Path string } // user pressed trash key
type DirLoadedMsg struct { Root string; Entries []Entry }  // user navigation
type DirReloadedMsg struct { Root string; Entries []Entry } // disk-triggered (fsnotify)
```

#### Entry Model

```go
type Entry struct {
    Name  string
    Path  string
    IsDir bool
}
```

#### Visual Appearance

- **Row 0**: Pane title showing the root directory path, rendered with `styles.PaneTitle` (bold, color 153 -- `Special`). Path is truncated with `…` prefix if wider than pane width.
- **Rows 1+**: Each entry on its own line:
  - **Selected (focused)**: `>` prefix + `styles.FileSelected` (bold, color 111 -- `Highlight`, padding 0,1)
  - **Not selected**: ` ` prefix + `styles.FileNormal` (color 252, padding 0,1)
  - Directory entries have `/` appended to the name
- **Outer wrapper**: `styles.Clip(width, height)` -- `MaxWidth(w).MaxHeight(h)`

#### Behavior

- **Keyboard (when focused)**:
  - `Up` / `Down`: Move cursor, clamped to entry bounds
  - `Home` / `End`: Jump to first/last entry
  - `Enter` (`PrimaryAction`): Emit `FileSelectedMsg` or `DirSelectedMsg` depending on `IsDir`
  - `TrashFile` (super+backspace / delete): Emit `FileDeleteRequestedMsg` (never for `..` parent entry)
  - **Fuzzy letter search**: Any printable character accumulates into `searchQuery` (case-insensitive prefix match). Typing faster than 750ms resets the query. Jump to first matching entry.
- **Mouse (when focused)**:
  - Left click: If on current cursor, emit selection msg. Otherwise move cursor to clicked row.
  - Wheel: Move cursor by 3 lines (from `listnav.WheelLines`).
- **Scroll**: `ensureVisible()` uses `listnav.Follow` with margin = min(4, size/4) for smooth scrolling.
- **Dir load vs reload**: `DirLoadedMsg` resets cursor to 0; `DirReloadedMsg` preserves cursor position (attempting to keep the same-named entry selected).

#### Dependencies

- `keymap.Bindings` -- Up, Down, GotoTop, GotoBottom, PrimaryAction, TrashFile
- `styles.Styles` -- PaneTitle, FileNormal, FileSelected, Clip
- `listnav.List` -- Cursor/Top navigation, Window, Follow, Wheel, ClickIndex
- `vfs.NormPath` -- path normalization for the root display

---

### 2. Open Tabs

**Go source:** `/Users/xiii/Developer/rune/pkg/ui/components/opentabs/`

#### Public API

```go
type Model struct { /* tabs, nav, activeHandle, activitySeq, width, height, offsetX, offsetY, focused, keys, styles */ }

type Tab struct {
    DocID         int64
    Path          string
    Name          string
    Pinned        bool
    Dirty         bool
    lastActiveSeq int64  // monotonic counter; 0 = never focused
}

type TabHandle struct { DocID int64; Path string }

func New(keys keymap.Bindings, st styles.Styles) Model
func (m Model) SetSize(w, h int) Model
func (m Model) SetOffset(x, y int) Model
func (m Model) SetFocused(f bool) Model
func (m Model) Focused() bool
func (m Model) Cursor() int
func (m Model) Len() int
func (m Model) Height() int           // len(tabs) + 1 (header row)
func (m Model) PathAt(index int) string
func (m Model) DocIDAt(index int) int64
func (m Model) AllDocIDs() []int64     // excludes DocID==0 (help)
func (m Model) SelectIndex(index int) Model
func (m Model) SetActive(h TabHandle) Model  // stamps outgoing tab's lastActiveSeq
func (m Model) ActiveHandle() TabHandle
func (m Model) PinIndex(index int) Model
func (m Model) OpenFile(docID int64, path string) Model  // add tab if absent; detach collision
func (m Model) AssignDocID(path string, docID int64) Model
func (m Model) NameOf(h TabHandle) string
func (m Model) HasUntitledPlaceholder() bool
func (m Model) HasTabNamed(name string) bool
func (m Model) SetName(h TabHandle, name string) Model
func (m Model) SetDirty(h TabHandle, dirty bool) Model
func (m Model) HasDirty() bool
func (m Model) DirtyTabs() []TabHandle
func (m Model) NeighborOf(h TabHandle) (TabHandle, bool)  // tab to switch to after close
func (m Model) Close(h TabHandle) Model
func (m Model) RenameFile(oldPath, newPath string) (Model, bool)
func (m Model) EvictionCandidate() (TabHandle, dirty, ok)  // LRU, clean preferred
func (m Model) HasTab(docID int64, path string) bool
func (m Model) Init() tea.Cmd
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)
func (m Model) View() string
```

#### Messages

```go
type TabSelectedMsg struct { DocID int64; Path string }
```

#### Visual Appearance

- **Row 0**: Divider header `"── Open ──────"` rendered with `styles.TabsDivider` (color 241 -- `Subtle`).
- **Rows 1+**: Each tab on its own line:
  - **Prefix**: `> ` when cursor is on this tab and focused; `  ` otherwise.
  - **Digit shortcut**: `(i+1)%10` followed by `:` (e.g., `1:`, `2:`, ..., `9:`, `0:`).
  - **Dirty indicator**: `x` in `styles.TabDirty` (color 196 -- `Error`/red) if dirty; ` ` (space) if clean.
  - **Space separator**: Single space after dirty indicator.
  - **Pinned prefix**: `★` in `styles.TabPinned` (color 153 -- `Special`) + space, if pinned.
  - **Tab name**: Rendered with `styles.TabActive` (bold, color 111 -- `Highlight`, padding 0,1) if active; `styles.TabNormal` (color 241 -- `Subtle`, padding 0,1) otherwise.
- **Outer wrapper**: `styles.Clip(width, height)`

#### Behavior

- **Tab identity**: `TabHandle.Equal` -- if either DocID != 0, compare by DocID (rename-safe). If both DocID == 0, compare by Path (for virtual docs: help, pre-store untitled).
- **`findTab` (asymmetric)**: h.DocID != 0 -> match stored tab's DocID. h.DocID == 0 -> match stored tab's Path unconditionally.
- **`SetActive`**: Stamps the outgoing tab's `lastActiveSeq` with a monotonically increasing counter. Moves cursor to the new active tab.
- **`OpenFile`**: If docID != 0 and a tab with that DocID exists, update path/name. If docID == 0, only add if no DocID==0 + same path tab exists. **Detaches** any other tab holding the same path (sets its Path to "").
- **`Close`**: Removes the tab. Clamps cursor. Resyncs cursor to active tab's new index.
- **`RenameFile`**: Updates path/name. If newPath already belongs to a different tab, detaches that tab. Returns `ok=false` if a collision was reconciled.
- **`EvictionCandidate`**: Among non-active, non-pinned, file-backed tabs: prefer clean over dirty; within each tier, pick the LRU (smallest lastActiveSeq).
- **Keyboard (when focused)**:
  - `Up` / `Down`: Move cursor
  - `Enter`: Emit `TabSelectedMsg`
- **Mouse (when focused)**:
  - Left click: Move cursor to clicked tab, emit `TabSelectedMsg`
- **Scroll**: `ensureVisible()` uses `listnav.Follow` with margin=0, jump=0 (short tab list doesn't need hysteresis).

#### Dependencies

- `keymap.Bindings` -- Up, Down, PrimaryAction
- `styles.Styles` -- TabsDivider, TabNormal, TabActive, TabPinned, TabDirty, Clip
- `listnav.List` -- Cursor/Top navigation, Window, Follow, ClickIndex

---

### 3. Footer

**Go source:** `/Users/xiii/Developer/rune/pkg/ui/components/footer/`

#### Public API

```go
type Model struct { /* line, col, wordCount, width, styles, keys, pendingKey, guardKind, guardOptions, guardLabel, dictating, dictationAllowed, mergeActive, mergeLeft, diskChanged, degraded, ephemeral, errorMsg, errorExpireID, statusMsg, statusExpireID, linkHint */ }

type GuardKind int  // Dirty, Merge, Trash, Deleted, Raced, Degraded
type DataLossGuardResponse int  // Save, Discard, Cancel, MergeAccept, MergeReject, Trash, SaveAnyway, Merge, KeepMine, RestoreTheirs, ConfirmDegraded

func New(keys keymap.Bindings, st styles.Styles) Model
func (m Model) SetSize(w, h int) Model
func (m Model) Height() int  // always 1
func (m Model) SetGuard(kind GuardKind, options []GuardOption) Model
func (m Model) SetGuardLabel(label string) Model
func (m Model) InGuard() bool
func (m Model) GuardKind() GuardKind
func (m Model) SetDictationAllowed(allowed bool) Model
func (m Model) SetDictating(active bool) Model
func (m Model) SetMergeMode(active bool, conflictsLeft int) Model
func (m Model) SetDiskChanged(changed bool) Model
func (m Model) SetDegraded(degraded bool) Model
func (m Model) Degraded() bool
func (m Model) SetEphemeral(ephemeral bool) Model
func (m Model) Ephemeral() bool
func (m Model) Init() tea.Cmd
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)
func (m Model) View() string
```

#### Messages

```go
type ConfirmQuitMsg struct{}
type ShowErrorMsg struct{ Text string }
type ShowStatusMsg struct{ Text string }
type UpdateCursorMsg struct { Line int; Col int; WordCount int; LinkTarget string }
type DataLossGuardResponseMsg struct { Response DataLossGuardResponse }
type DictationStartMsg struct{}
type DictationStopMsg struct{}
```

#### Display Modes (priority order, highest first)

The footer has a single `displayMode()` function that determines what the left side shows:

| Priority | Mode | Condition | Left Side Content |
|----------|------|-----------|-------------------|
| 1 | `modeError` | `errorMsg != ""` | **Full width**: `⚠ <error text>` in `styles.Error` (color 196, bold). Replaces entire footer content (no Ln/Col). |
| 2 | `modeDictating` | `dictating` | `^v` (FooterKey) + ` stop dictation` (FooterHint) |
| 3 | `modeGuard` | `guardVisible()` | Guard label + `[Key]suffix` options (e.g., `Unsaved changes. [S]ave [D]iscard [Esc] Cancel`) |
| 4 | `modeChordPending` | `pendingKey in {c, d}` | `Press ^C again to exit` or `Press ^D again to exit` (FooterKey) |
| 5 | `modeMergeHint` | `mergeActive` | `⚙ Merge  [O]urs [T]heirs  ·  n/p next·prev  ·  N left` |
| 6 | `modeDiskChanged` | `diskChanged` | `⚠ File changed on disk` |
| 7 | `modeDegraded` | `degraded` | `⚠ Storage degraded — history will not survive a crash` |
| 8 | `modeEphemeral` | `ephemeral` | `◌ In-memory session — nothing recorded; explicit saves still write to disk` |
| 9 | `modeStatus` | `statusMsg != ""` | The status text (FooterHint) |
| 10 | `modeLinkHint` | `linkHint != ""` | `→ <link target>  ⏎ open` |
| 11 | `modeDefault` | (nothing above) | `<^x> explorer  <^e> editor  <^r> chat  <F1> help` (keys from bindings, FooterKey + FooterHint) |

#### Visual Appearance

- **Background**: `styles.Footer` -- `Background(Surface)` where Surface = color 236, `Padding(0, 1)`.
- **FooterKey**: `Foreground(Highlight)` (color 111), `Background(Surface)` (236), `Bold(true)`.
- **FooterHint**: `Foreground(Subtle)` (color 241), `Background(Surface)` (236).
- **FooterMeta**: `Foreground(Special)` (color 153), `Background(Surface)` (236).
- **Error**: `Foreground(Error)` (color 196), `Bold(true)`.
- **Layout**: `<left content><spaces><right content>` where right = `Ln <n>, Col <n>  W:<words>  🎤`. The gap is computed as `innerWidth - width(left) - width(right)`, minimum 1.
- **Mic icon**: `🎤` in FooterMeta. When dictating: `🎤 ●` in FooterKey.
- **Height**: Always exactly 1 row.

#### Behavior

- **Chord quit confirmation**: `^C^C` or `^D^D` (same chord twice within 2s window). First press arms `pendingKey`, spawns 2s timer. Same chord again emits `ConfirmQuitMsg`. Different chord re-arms. Timer expiry clears `pendingKey`.
- **Guard mode**: When `guardOptions` is non-empty, ALL keypresses are consumed by the footer. Typed key matching an option's `Key` resolves the guard. Escape resolves to the last option. Enter on Merge/Deleted guards maps to Cancel (last option).
- **Error auto-dismiss**: `ShowErrorMsg` sets `errorMsg`, spawns a 5s timer. `errorDismissedMsg` clears if generation matches.
- **Status auto-dismiss**: Same pattern as error, 5s timer.
- **Cursor update**: `UpdateCursorMsg` updates line, col, wordCount, linkHint.

#### Dependencies

- `keymap.Bindings` -- ConfirmExitC, ConfirmExitD, VoiceDictation, Cancel, FocusExplorer, FocusEditor, FocusChat, Help
- `styles.Styles` -- Footer, FooterKey, FooterHint, FooterMeta, Error

---

## Style Token Map

### Color Palette (from `pkg/ui/styles/styles.go:104-112`)

| Token | Value | Usage |
|-------|-------|-------|
| `Subtle` | `"241"` (dark gray) | Inactive text, dividers, separators, inactive tabs |
| `Highlight` | `"111"` (light blue-gray) | Active/focused elements, selected items, footer keys |
| `Special` | `"153"` (teal) | Pane titles, pinned tabs, breadcrumb, footer meta |
| `Error` | `"196"` (red) | Error messages, dirty tab indicators |
| `Surface` | `"236"` (very dark gray) | Footer background, inline code background, code block background |
| `CodeBg` | `"235"` (darker gray) | Code block background |

### Additional Colors Used Inline

| Color | Value | Usage |
|-------|-------|-------|
| `"252"` (white) | File normal text, code plain text, table |
| `"23"` (red) | H1 background |
| `"230"` (white) | H1 foreground |
| `"63"` (cyan) | H3 foreground |
| `"39"` (green) | H4 foreground |
| `"245"` (gray) | Code comment, blockquote, list marker, H6 |
| `"114"` (green) | Code string, task checked |
| `"177"` (magenta) | Code type |
| `"216"` (yellow) | Code number, title text |
| `"240"` (dark gray) | Table border, horizontal rule, table separator, task unchecked |
| `"108"` (green) | Tag foreground |
| `"58"` (blue) | Search match background |
| `"130"` (purple) | Search active match background |
| `"239"` (dark gray) | Selection background |
| `"22"` (dark green) | Merge ours background |
| `"52"` (dark red) | Merge theirs background |

### Border Style (from `styles.go:115-117`)

```go
border := lipgloss.NewStyle().
    Border(lipgloss.RoundedBorder()).
    BorderForeground(subtle)  // color 241
```

- **Active border**: Same as base, but `BorderForeground(highlight)` (color 111)
- **Inactive border**: Base (color 241)
- **Border type**: `RoundedBorder` -- `╭──╮` / `│  │` / `╰──╯`

### Component Styles Summary

| Style | Foreground | Background | Modifiers | Padding |
|-------|-----------|------------|-----------|---------|
| `PaneTitle` | 153 (Special) | -- | Bold | -- |
| `FileNormal` | 252 | -- | -- | 0,1 |
| `FileSelected` | 111 (Highlight) | -- | Bold | 0,1 |
| `TabsDivider` | 241 (Subtle) | -- | -- | -- |
| `TabNormal` | 241 (Subtle) | -- | -- | 0,1 |
| `TabActive` | 111 (Highlight) | -- | Bold | 0,1 |
| `TabPinned` | 153 (Special) | -- | -- | -- |
| `TabDirty` | 196 (Error) | -- | -- | -- |
| `Footer` | -- | 236 (Surface) | -- | 0,1 |
| `FooterKey` | 111 (Highlight) | 236 (Surface) | Bold | -- |
| `FooterHint` | 241 (Subtle) | 236 (Surface) | -- | -- |
| `FooterMeta` | 153 (Special) | 236 (Surface) | -- | -- |
| `Error` | 196 (Error) | -- | Bold | -- |

### Symbols and Characters

| Symbol | Unicode | Usage |
|--------|---------|-------|
| Dirty dot | `•` (U+2022) | Status line dirty indicator (Rust: `status.rs:13`) |
| Pinned star | `★` (U+2605) | Pinned tab prefix |
| Warning | `⚠` (U+26A0) | Error messages, disk changed, degraded storage |
| In-memory | `◌` (U+25CC) | Ephemeral session banner |
| Microphone | `🎤` (U+1F3A4) | Dictation indicator |
| Recording dot | `●` (U+25CF) | Active dictation |
| Arrow | `→` (U+2192) | Link hint prefix |
| Enter | `⏎` (U+23CE) | Link open hint |
| Merge gear | `⚙` (U+2699) | Merge resolver hint |
| Truncation | `…` (U+2026) | Path truncation prefix |
| Cursor prefix | `>` (U+003E) | Selected item in filetree/tabs |
| Divider | `──` (U+2500 × 2) | Open tabs header, breadcrumb fill |
| Border corners | `╭╮╰╯` (U+256D-F) | Rounded border (lipgloss RoundedBorder) |
| Border vertical | `│` (U+2502) | Rounded border sides |

---

## Gap Analysis

### Rust TUI Framework

The Rust project uses **ratatui 0.30.2** with **termina 0.3.3** backend (via ratatui-termina 0.1.0). This is the modern fork of the tui-rs ecosystem. Key differences from Go's Bubble Tea + lipgloss:

| Aspect | Go (Bubble Tea + lipgloss) | Rust (ratatui + termina) |
|--------|---------------------------|--------------------------|
| Rendering model | String-based: each component returns a `View() string`, parent composes | Widget-based: `Frame::render_widget()` draws into a shared buffer |
| Styling | lipgloss `Style` struct with chainable methods | ratatui `Style` with `fg()`, `bg()`, `add_modifier()` |
| Layout | lipgloss `JoinHorizontal`/`JoinVertical`, `Width()`, `Height()` | ratatui `Layout` with `Constraint`, `Direction`, `split()` |
| Text | Plain strings with ANSI | `Line`, `Span`, `Text` types |
| Borders | lipgloss `Border()` + `BorderType` | ratatui `BorderSet` + `BorderType` |
| Terminal I/O | Bubble Tea handles it | `termina` for events, `ratatui::Terminal` for draw |

**Implication**: The Go components' `View() string` pattern (build a string, let lipgloss render it) does not directly map to ratatui's widget model. The Rust port needs to either:
1. **Widget approach**: Build proper ratatui widgets (List, Block, Paragraph, etc.) -- idiomatic ratatui but requires learning the widget API.
2. **String-then-blit approach**: Build strings like Go does, then use ratatui's `buffer_mut()` to write cells directly -- this is what the current `render.rs` already does for the editor pane (the `blit()` function).

The current Rust codebase already uses approach (2) for the editor pane. For consistency, the new components should follow the same pattern: build the visual content as `Vec<Vec<Cell>>` (or equivalent), then blit into the frame buffer.

### What Needs to Be Built

#### Foundation Layer (prerequisite for all three components)

1. **Styles system** -- A Rust equivalent of Go's `styles.Styles` struct. This is the single most important prerequisite. Every component references it. Define a `Palette` struct and derive all component styles from it. Map lipgloss concepts to ratatui `Style`:
   - `Foreground(color)` -> `Style::fg(Color::xxx)`
   - `Background(color)` -> `Style::bg(Color::xxx)`
   - `Bold(true)` -> `Style::add_modifier(Modifier::BOLD)`
   - `Padding(0, 1)` -> handled in the blit/layout layer (ratatui doesn't have lipgloss-style padding on arbitrary text)

2. **List navigation abstraction** -- Port `pkg/ui/listnav/listnav.go` as a shared `listnav` module. The `List` struct (Cursor, Top) with `Move`, `First`, `Last`, `Follow`, `Window`, `Wheel`, `ClickIndex`. This is used by both filetree and opentabs.

3. **Scroll module** -- Port `pkg/ui/scroll/scroll.go` (the `Follow` function that listnav depends on for viewport tracking with margin/jump).

4. **Multi-pane layout** -- Replace `render::draw()`'s current two-section split (editor + status) with a layout system that supports left pane (filetree + opentabs), center pane (title + search + editor), right pane (chat), and footer. The Go `paneGeometry()` function in `workspace_view.go:22-119` is the reference.

#### Component Layer

5. **Explorer (filetree)** -- New module. State: `Vec<Entry>`, `listnav::List`, root path, dimensions, focus, search query. Events: directory load/reload messages, selection messages, delete request. Must integrate with VFS for directory reading.

6. **Open Tabs** -- New module. State: `Vec<Tab>`, `listnav::List`, active handle, activity sequence, dimensions, focus. Must integrate with the document identity system (DocID). Eviction candidate logic for tab limit enforcement.

7. **Footer** -- Replace `status.rs` with a full footer module. Display mode priority table. Guard state machine. Chord quit confirmation. Auto-dismiss timers for errors/status. Cursor position display. Dictation indicator. Merge resolver hint. Default keybinding hints.

#### Integration Layer

8. **Workspace focus routing** -- A `focus` enum (Tree, Tabs, Center, Title, Chat, Search) on the App. Key routing logic that dispatches keys to the focused component. Global key interception (save, close, new, focus switches). The Go `workspace_update_keys.go` is the reference.

9. **Pane focus indicators** -- Active pane gets `ActiveBorder` (color 111), inactive gets `InactiveBorder` (color 241). The border color change is the visual focus indicator.

---

## Recommended Implementation Order

### Phase A: Foundation (enables everything else)

1. **Styles module** (`rune-tui/src/styles.rs`)
   - Define `Palette` struct with the 6 base colors
   - Define derived style functions (one per style token: `pane_title()`, `file_normal()`, `file_selected()`, `tabs_divider()`, `tab_normal()`, `tab_active()`, `tab_pinned()`, `tab_dirty()`, `footer()`, `footer_key()`, `footer_hint()`, `footer_meta()`, `error()`, `active_border()`, `inactive_border()`)
   - Map 256-color palette values to ratatui `Color` (most map directly via `Color::AnsiValue(n)`)

2. **Listnav module** (`rune-tui/src/listnav.rs`)
   - Port `List` struct, `Move`, `First`, `Last`, `Follow`, `Window`, `Wheel`, `ClickIndex`
   - The `Follow` function needs the scroll module's `Follow` -- either inline it or create a separate `scroll.rs`

3. **Multi-pane layout** (modify `rune-tui/src/render.rs`)
   - Add layout constants: `DEFAULT_LEFT_PANE_W = 22`, `MIN_LEFT_PANE_W = 16`, `MIN_CENTER_W = 24`
   - Replace the current two-section split with a three-column + footer layout
   - The left pane is initially hidden (controlled by a `left_visible` flag on App)
   - Each pane gets a rounded border with active/inactive color

### Phase B: Components (can be done in parallel after Phase A)

4. **Footer** (`rune-tui/src/footer.rs`, replacing `status.rs`)
   - Start with `modeDefault` (keybinding hints) and `modeError` -- these have the most visible impact
   - Add `modeChordPending` (quit confirm) -- already partially implemented in `status.rs`
   - Add `modeStatus` (transient status messages with auto-dismiss)
   - Add `UpdateCursorMsg` handling for Ln/Col/word count display
   - Add guard modes as needed by workspace integration

5. **Explorer** (`rune-tui/src/explorer.rs`)
   - Start with directory display and cursor navigation
   - Add Enter selection, mouse click, wheel scroll
   - Add fuzzy letter search
   - Integrate with VFS `read_dir` for directory loading

6. **Open Tabs** (`rune-tui/src/opentabs.rs`)
   - Start with tab display and cursor navigation
   - Add Enter selection, mouse click
   - Add dirty indicator, pinned star
   - Add `SetActive`, `Close`, `OpenFile` API
   - Add eviction candidate logic

### Phase C: Integration

7. **Workspace focus routing** (modify `rune-tui/src/app.rs`)
   - Add `focus` enum to App
   - Add `set_focus` method
   - Route keys to focused component in `handle_key`
   - Add global key interception (FocusExplorer, FocusEditor, ZenMode)
   - Wire `^x` to show/hide left pane and focus filetree

8. **Message integration**
   - Add `FileSelectedMsg`, `DirSelectedMsg`, `TabSelectedMsg` to `Msg` enum
   - Handle directory loading on file selection (VFS read_dir -> `DirLoadedMsg`)
   - Handle tab switching (load document content)

---

## Citations

### Go Source Files

| File | Lines | Content |
|------|-------|---------|
| `pkg/ui/styles/styles.go` | 1-203 | Complete styles system: Palette, Styles struct, Default() with all colors |
| `pkg/ui/components/filetree/filetree.go` | 1-252 | Filetree model, Update, View, renderFileList, truncatePath |
| `pkg/ui/components/filetree/item.go` | 1-9 | Entry struct |
| `pkg/ui/components/filetree/mouse.go` | 1-51 | Mouse click and wheel handling |
| `pkg/ui/components/opentabs/opentabs.go` | 1-473 | Tabs model, Tab/TabHandle, all public API, View, handleMouseClick |
| `pkg/ui/components/opentabs/eviction.go` | 1-74 | EvictionCandidate, HasTab |
| `pkg/ui/components/opentabs/opentabs_close.go` | 1-82 | Close, RenameFile, resyncCursorAfterRemoval |
| `pkg/ui/components/footer/footer.go` | 1-245 | Footer model, Update, guard types, confirmChord, timers |
| `pkg/ui/components/footer/footer_view.go` | 1-189 | Display modes, renderLeft, View |
| `pkg/ui/components/footer/footer_guard.go` | 1-160 | GuardKind, GuardOption, guardDescriptorFor, renderGuardHint |
| `pkg/ui/components/footer/footer_timers.go` | 1-9 | Timer constants (5s error dismiss, 2s confirm) |
| `pkg/ui/listnav/listnav.go` | 1-116 | List struct, Move, First, Last, Follow, Window, Wheel, ClickIndex |
| `pkg/ui/keymap/keymap.go` | 1-348 | Bindings struct, Default() with all keybindings |
| `pkg/ui/pages/workspace/workspace.go` | 1-434 | Workspace model, New, Init, pane enum, layout constants |
| `pkg/ui/pages/workspace/workspace_view.go` | 1-476 | paneGeometry, recalcLayout, View, borderStyle, overlayBreadcrumb |
| `pkg/ui/pages/workspace/workspace_update_keys.go` | 1-314 | handleKeyPress, global key routing, focus projection |
| `pkg/ui/pages/workspace/workspace_nav.go` | 1-393 | requestOpenPath, toggleHelp, CreateUntitled, requestCloseCurrent |

### Rust Source Files

| File | Lines | Content |
|------|-------|---------|
| `crates/rune-tui/src/lib.rs` | 1-14 | Module declarations |
| `crates/rune-tui/src/app.rs` | 1-849 | App model, update, handle_key, quit-confirm, save, tests |
| `crates/rune-tui/src/editor.rs` | 1-182 | Editor, Viewport, sync, scroll_to_cursor |
| `crates/rune-tui/src/render.rs` | 1-328 | Cell, style_for, segment_cells, build_rows, blit, draw |
| `crates/rune-tui/src/status.rs` | 1-84 | Status line (minimal), status_text, draw |
| `crates/rune-tui/src/runtime.rs` | 1-214 | Msg, Cmd, Effects, run, apply, spawn_cmd, spawn_input_reader |
| `crates/rune-tui/src/term.rs` | 1-156 | Guard, terminal lifecycle, Kitty flags |
| `crates/rune-tui/src/keymap.rs` | 1-429 | KeyCode, Mods, KeyInput, Command, resolve, QuitKey |
| `crates/rune-tui/src/clipboard.rs` | 1-83 | OSC 52 copy, pbpaste_cmd |
| `crates/rune-tui/Cargo.toml` | 1-19 | Dependencies: ratatui, ratatui-termina, termina |
| `Cargo.toml` | 1-35 | Workspace: members, dependencies, lints |
| `ROADMAP.md` | 1-41 | Project roadmap (items 1-8) |
| `DATABASE MODEL.md` | 1-138 | SQLite architecture decision document |
