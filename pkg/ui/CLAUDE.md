# Rune — TUI Architecture (`pkg/ui/`)

This file extends the repository-root `CLAUDE.md` and governs all code under `pkg/ui/` — the only Bubble Tea / Elm-cycle code in the repo. It carries §2–§7 plus the UI rows of the §9 pitfalls table and the §11 checklist.

The root `CLAUDE.md` loads alongside this file; its §0 (Prime Directive) and §1 (Go fundamentals) bind here too — read them first, this file doesn't repeat them. References to §0/§1/§8/§10 point to the root file.

Section numbers match the original monolithic `CLAUDE.md` and are **stable** — source code and ADRs cite them by number (e.g. §3.2, §6.3). Don't renumber.

---

## 2. Component Architecture

Most architectural defects start here.

### 2.1 State Residency Rule

> **The component that RENDERS a piece of state OWNS that state on its Model.**

Litmus: if deleting a child's `View()` call would make a parent field dead code, that field belongs on the child.

```go
// ✓ page holds the child by value, untouched
type Model struct { editor editor.Model }       // pages/workspace
// ✗ page holding viewport / openPath / breadcrumb — those are the editor's render state
```

### 2.2 Component Contract

Every component exposes this contract as concrete methods (not Go interfaces — avoid boxing):

```go
func New(keys keymap.Bindings, st styles.Styles) Model // constructor with deps
func (m Model) Init() tea.Cmd
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)
func (m Model) View() string                           // pure render (or tea.View)
func (m Model) SetSize(w, h int) Model                  // accept allocated dimensions
func (m Model) Height() int                             // intrinsic/allocated height
```

- `SetSize` returns a new Model (value semantics); the component stores its dimensions and uses them in `View()`.
- `Height()` returns intrinsic or allocated height. Parents query it; they never hardcode it.
- A component that manages focus also exposes `SetFocused(bool) Model`.

### 2.3 Pages vs Components

| Concern | Page | Component |
|---------|------|-----------|
| Holds child Models | ✓ by value | ✓ by value (sub-components) |
| Handles focus routing | ✓ | ✗ (receives `SetFocused`) |
| Translates cross-component messages | ✓ | ✗ |
| Owns rendering state (text, content, scroll) | ✗ | ✓ |
| Defines layout geometry | ✓ queries children, allocates space | ✗ accepts via `SetSize` |
| Emits I/O commands (file reads, network) | ✓ orchestrates | ✓ for component-owned I/O |
| Handles global keybindings (quit, focus change) | ✓ | ✗ |
| Handles scoped keybindings (navigation, selection) | ✗ forwards to focused child | ✓ when `m.focused` |

**Extraction rule:** if a page's `View()` composes multiple sub-views into a logical unit (breadcrumb + viewport = "editor pane"), extract that unit into a component. Don't inline a component's rendering in a page.

### 2.4 Message Ownership & Flow

Define a message in the **producer's** package — the component that generates the event or owns the data it carries.

| Message type | Defined in | Consumed by |
|---|---|---|
| `FileSelectedMsg{Path}` | `components/filetree` | page (orchestrates file load) |
| `FileLoadedMsg{Path, Content}` | `components/editor` | `components/editor` (owns viewport content) |
| `TabSelectedMsg{Path}` | `components/opentabs` | page (orchestrates file load) |
| `ConfirmQuitMsg` | `components/footer` | page (calls `tea.Quit`) |

If a message carries data only ONE component renders, define it in that component's package. Same for a Cmd factory: if its message has a single consumer, both factory and message type live in that component's package — the page calls the factory, it doesn't implement the I/O.

```go
// Page receives from child A, translates, forwards to child B
case filetree.FileSelectedMsg:
    return m, editor.LoadFileCmd(msg.Path)   // editor defines + handles its own load
case editor.FileLoadedMsg:                   // editor handled it internally; page forwards
    m.opentabs, cmd = m.opentabs.Update(opentabs.FileOpenedMsg{Path: msg.Path})
```

Keep timer/expiry messages for internal state machines (e.g. chord timeout) as unexported types in the owning component; export only the final result message (e.g. `ConfirmQuitMsg`).

### 2.5 Dependency Injection

- Pass `keymap.Bindings` and `styles.Styles` by value via constructors.
- Pass a `context.Context` (see §6) or `*log.Logger` into the `tea.Cmd` factory that needs it — keep both off every Model field.
- Shared UI environment (styles, keymaps) flows top-down through constructors. Components import neither pages nor domain packages.

---

## 3. Keybinding Architecture

### 3.1 Binding Organization

All keybindings live in `keymap.Bindings` (one struct) — the single source of truth. Give each key string exactly one binding: before adding a key, scan every `key.WithKeys(...)` in `Default()` and confirm no other binding uses it. One physical keypress resolves to exactly one logical action; a duplicate is a defect.

### 3.2 Chord & Multi-Press Keybindings

A chord is a multi-press sequence (e.g. `^C` twice to quit). Own the chord state machine in the component that **renders the confirmation feedback**.

```go
// components/footer — owns the confirm-exit chord
type ConfirmQuitMsg struct{} // exported: emitted when the chord completes
type confirmExpired struct{} // unexported: internal timeout

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
    switch msg := msg.(type) {
    case tea.KeyPressMsg:
        if key.Matches(msg, m.keys.ConfirmExitC) {
            if m.pendingKey == "c" {              // second press completes the chord
                m.pendingKey = ""
                return m, func() tea.Msg { return ConfirmQuitMsg{} }
            }
            m.pendingKey = "c"                     // first press arms + starts the timer
            return m, m.startConfirmTimer()
        }
    case confirmExpired:
        m.pendingKey = ""
    }
    return m, nil
}
// startConfirmTimer: a tea.Cmd that sleeps ~2s then returns confirmExpired{}.
// View shows "Press ^C again to exit" while m.pendingKey == "c".
// The page handles only the result:  case footer.ConfirmQuitMsg: return m, tea.Quit
```

A page doesn't render the prompt, so it doesn't own the pending state and doesn't match chord-sequence bindings (`ConfirmExitC`, etc.) in its own `KeyPressMsg` switch. It forwards the `KeyPressMsg`; the owning component matches internally and emits the result message on completion.

### 3.3 Focus-Scoped Key Handling

Handle global keys FIRST in the page, then forward the message to all children, each gating on `m.focused`.

```go
// Page Update
case tea.KeyPressMsg:
    switch { // global keys — handled regardless of focus
    case key.Matches(msg, m.keys.FocusExplorer): m.focus = paneTree
    case key.Matches(msg, m.keys.ZenMode):       m.leftVisible = !m.leftVisible
    }
    // global keys stop here (return early / use a consumed flag — don't forward them)
// then forward, letting children gate on focus:
//   m.filetree = m.filetree.SetFocused(m.focus == paneTree)
//   m.filetree, cmd = m.filetree.Update(msg)

// Inside a component
case tea.KeyPressMsg:
    if !m.focused { return m, nil } // ignore keys when not focused
    // handle navigation keys...
```

### 3.4 Key Matching Order

`key.Matches` runs sequentially, so order matters:

1. **Chord completions** — check pending state first (most specific).
2. **Global actions** — quit, focus changes, zen mode.
3. **Contextual actions** — tab switch, pin, close (state-dependent).
4. **Fallthrough to children** — navigation, selection.

---

## 4. Layout & Dimensions

### 4.1 Intrinsic vs Extrinsic Sizing

- **Intrinsic:** a component knows its own natural size (footer = 1 line; opentabs = `len(tabs)+1`). Expose via `Height()`.
- **Extrinsic:** a parent allocates space (room minus children's intrinsic sizes), passed via `SetSize(w, h)`.

Query the component for a dimension it owns — never a package-level `const`:

```go
contentH := totalH - m.footer.Height()   // ✓ query the component
leftW := m.sidebar.PreferredWidth()       // ✓ a method, or a configurable field
// const footerH = 1; const leftPaneW = 22 ✗ magic numbers for a child's dimension
```

### 4.2 Dimension Calculation Flow

Do all dimension math in `recalcLayout()`, called from `Update` after `tea.WindowSizeMsg` OR any structural change (pane toggled, tab added/removed): query children's intrinsic sizes → compute allocations → `SetSize(w,h)` every child → return the model.

```go
func (m Model) recalcLayout() Model {
    contentH := m.totalHeight - m.footer.Height()
    // ... compute leftW, centerW from visibility + intrinsic widths
    m.editor   = m.editor.SetSize(centerW, contentH)
    m.filetree = m.filetree.SetSize(leftW, contentH)
    m.footer   = m.footer.SetSize(m.totalWidth, m.footer.Height())
    return m
}
```

### 4.3 Border Arithmetic — lipgloss v2 is Border-Box

In lipgloss v2, `Width(n)`/`Height(n)` are **border-box**: `n` is the **total** rendered dimension including borders and padding; the content area is `n - frame_size`. `Height(n)` is minimum-only — it pads short content but never truncates tall content.

Subtract the frame ONCE — in `recalcLayout()`, when computing child dimensions. The border style in `View()` gets the OUTER dimension; children get INNER (outer − frame):

```go
// recalcLayout — inner dimension for the child (subtract the frame ONCE)
innerH := contentH - 2
m.editor = m.editor.SetSize(innerCenterW, innerH)
// View — the border style gets the OUTER dimension
borderStyle.Width(centerW).Height(contentH).Render(m.editor.View())
// borderStyle.Width(centerW-2).Height(contentH-2) ✗ double-subtracts the frame
```

Use `GetHorizontalFrameSize()`/`GetVerticalFrameSize()` instead of a hardcoded `2` for the exact overhead.

### 4.4 Hard Dimension Clamping (`MaxWidth` / `MaxHeight`)

- `Height(n)` is a **minimum** (pads, never truncates); `Width(n)` **wraps** overflow beyond `n - frame_size` (adding visual lines).
- `MaxHeight(n)` / `MaxWidth(n)` **truncate** — hard-clamp to at most `n` lines / `n` cells (no wrapping).

Clamping is single-owner: the component that received `SetSize(w,h)` makes its `View()` fit `w×h`, via `MaxWidth(m.width).MaxHeight(m.height)` as the outermost style:

```go
func (m Model) View() string {
    content := /* ... render sub-views ... */
    return lipgloss.NewStyle().MaxWidth(m.width).MaxHeight(m.height).Render(content)
}
```

A page trusts a sized child's `View()` — it doesn't re-clamp:

```go
borderStyle.Render(m.editor.View())  // ✓ trust the child contract
// borderStyle.Render(lipgloss.NewStyle().MaxWidth(w).MaxHeight(h).Render(m.editor.View())) ✗ redundant, masks bugs
```

A page DOES use `Height`/`Width` on its own layout wrappers (e.g. border styles) to fill space — that's its own box-sizing, not child-clamping. The border's `Height(innerH)` pads the box when the child is shorter.

### 4.5 Rendering & Unicode

Every rendering calculation — span position, cell column, wrap boundary, table-cell width — counts **display characters**, not bytes (root §1.5):

```go
cellEnd := cellStart + utf8.RuneCountInString(cellText) // ✓ rune-based offset
// for i := 0; i < len(sp.Text); i++ { … }              ✗ byte offsets break on CJK/emoji/accents
```

If the calculation affects what the user sees, use `utf8.RuneCountInString`. Purely internal offsets (buffer, syntax tree) may use `len()` — but double-check the assumption.

---

## 5. The Elm Cycle

### 5.1 Value Receivers — No Exceptions

`Init`, `Update`, `View` use value receivers — `func (m Model) Update(...)`, not `func (m *Model) Update(...)`. Don't add a pointer receiver to satisfy an interface; if a bubbles library needs the pointer-based `tea.Model`, wrap it with a thin adapter at the boundary rather than infecting the component tree.

### 5.2 Pure View

`View()` is a pure function of Model state. It assigns nothing that outlives the call, does no I/O, calls no mutating method on child models (`SetWidth`/`SetHeight`), and returns/emits no `tea.Cmd`.

### 5.3 Non-Blocking Update

Keep `Update()` and `Init()` non-blocking. File I/O, network, and timers each become a `tea.Cmd` that returns a typed result `Msg` (a timer is a Cmd with `time.Sleep` inside that returns an expiry `Msg`). `time.Sleep` directly in `Update` is always wrong.

### 5.4 Synchronous State vs Async Commands

Use a `tea.Cmd` only for work that leaves the goroutine: file I/O, network, timers, syscalls. Mutate synchronous state directly in the same `Update` pass — don't wrap it in a Cmd:

```go
// ✓ mutate the child directly; only the I/O is a Cmd
if path := m.opentabs.PathAt(idx); path != "" {
    m.opentabs = m.opentabs.SelectIndex(idx)
    cmds = append(cmds, editor.LoadFileCmd(path))
}
// cmds = append(cmds, func() tea.Msg { return opentabs.TabSwitchIndexMsg{Index: idx} }) ✗ self-message
```

A self-messaging Cmd delays delivery by a frame, creates page→Cmd→runtime→page→child ping-pong, and leaks internal message types. If the page knows the intent and has the data, it acts directly via child methods. Components expose query + mutation methods for the page to orchestrate:

```go
func (m Model) PathAt(i int) string      // query: what file is at this index?
func (m Model) SelectIndex(i int) Model  // mutate: switch active tab
func (m Model) PinIndex(i int) Model     // mutate: toggle pin
func (m Model) OpenFile(p string) Model  // mutate: add/activate tab
func (m Model) CloseFile(p string) Model // mutate: remove tab
```

Components emit a message only to report a user-initiated action the page can't anticipate (e.g. Enter on a focused item):

```go
case key.Matches(msg, m.keys.Select):
    path := m.tabs[m.cursor].Path
    return m, func() tea.Msg { return TabSelectedMsg{Path: path} }
```

### 5.5 Cmd Closure Safety

Capture model-derived values into locals BEFORE the closure — it runs asynchronously, so the snapshot at creation is what matters (true even with value receivers):

```go
idx := m.opentabs.Cursor()                                          // ✓ capture into a local first
cmds = append(cmds, func() tea.Msg { return TabPinnedMsg{Index: idx} })
// func() tea.Msg { return TabPinnedMsg{Index: m.opentabs.Cursor()} } ✗ reads m at run time → race
```

### 5.6 Cmd Forwarding & Batching

After every `child.Update(msg)`, capture and accumulate the returned `tea.Cmd` — don't drop it by ignoring the second return. Return `tea.Batch(cmds...)`. `tea.WindowSizeMsg` reaches every child that stores dimensions.

```go
m.editor, cmd = m.editor.Update(msg); cmds = append(cmds, cmd) // accumulate every child Cmd
m.footer, cmd = m.footer.Update(msg); cmds = append(cmds, cmd)
return m, tea.Batch(cmds...)
```

### 5.7 Viewport Scroll Preservation

`viewport.SetContent()` is scroll-destructive — preserve the offset around it:

```go
wasAtBottom := m.viewport.AtBottom()
offset := m.viewport.YOffset
m.viewport.SetContent(newContent)
if wasAtBottom { m.viewport.GotoBottom() } else { m.viewport.SetYOffset(offset) }
```

---

## 6. Context & Async Operations

### 6.1 Context on Models

Keep `context.Context` off every Model field (and off any "shared"/`Common` struct):

```go
type Model struct { ctx context.Context } // ✗ never — even in a shared struct
```

### 6.2 Context in Cmd Closures

Pass context INTO a `tea.Cmd` factory for I/O cancellation:

```go
func loadFileCmd(ctx context.Context, fsys vfs.FS, path string) tea.Cmd {
    return func() tea.Msg {
        select {
        case <-ctx.Done(): return FileLoadCancelledMsg{Path: path}
        default:
        }
        b, err := fsys.ReadFile(path)   // through vfs.FS, never os.ReadFile (root §1.4.9)
        if err != nil { return ErrMsg{Err: fmt.Errorf("open %q: %w", path, err)} }
        return FileLoadedMsg{Path: path, Content: string(b)}
    }
}
```

### 6.3 Cancellation Pattern

Store a `context.CancelFunc` to cancel a previous operation (file watchers, long searches):

```go
type Model struct { cancelSearch context.CancelFunc }

func (m Model) startSearch(query string) (Model, tea.Cmd) {
    if m.cancelSearch != nil { m.cancelSearch() } // cancel previous
    ctx, cancel := context.WithCancel(context.Background())
    m.cancelSearch = cancel
    return m, searchCmd(ctx, query)
}
```

---

## 7. Page Transitions & Routing

### 7.1 Top-Level Router

`app.go`'s Model is a **router**: it holds zero rendering state and delegates to the active page.

```go
type Model struct {
    activePage page // enum or index
    workspace  workspace.Model
    settings   settings.Model
}
func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
    switch msg := msg.(type) {
    case NavigateMsg:
        m.activePage = msg.Page
        return m, m.activeModel().Init()
    }
    // ... delegate to the active page
}
```

### 7.2 Page Lifecycle

- A page's `Init()` runs when it becomes active (not at app startup for every page).
- Backgrounded pages retain state but don't receive messages.
- Only the active page's `View()` is called.

### 7.3 Inter-Page Communication

Pages communicate via messages routed through the top-level app:

```go
case editor.OpenSettingsMsg:
    m.activePage = pageSettings
    return m, m.settings.Init()
```

---

## 9. LLM-Specific Pitfalls (TUI)

Do the left — it's the fix for the middle. (Data-safety / Go pitfalls: root `CLAUDE.md` §9.)

| Do this | Instead of | § |
|---|---|---|
| Use a concrete value type for a component | a Go `interface` for a single concrete component | §2.2 |
| Value receivers on `Init`/`Update`/`View` | pointer receivers | §5.1 |
| Keep `context.Context`/`*log.Logger` off Models; pass into the Cmd | storing them on a Model | §6.1 |
| Define a message in the producer/owner package | defining it in the consumer | §2.4 |
| Let the child own state only it renders | a page holding it | §2.1 |
| Query the child's `Height()`/`Width()` | a package-level `const` for its dimension | §4.1 |
| Give each key string one binding | the same key in two `Bindings` fields | §3.1 |
| Capture model fields into locals before a `tea.Cmd` closure | reading `m.field`/`m.child.M()` inside it | §5.5 |
| Own chord/confirm state in the rendering component | implementing it in a page | §3.2 |
| Accumulate every child Cmd and `tea.Batch` them | returning `nil` and dropping child Cmds | §5.6 |
| Gate forwarded keys on `m.focused` in each child | forwarding `KeyPressMsg` to all children unconditionally | §3.3 |
| Run timers/sleep as a `tea.Cmd` | `time.Sleep` inside `Update()`/`Init()` | §5.3 |
| Extract a multi-view logical unit into a component | inlining it in a page's `View()` | §2.3 |
| Size children in `recalcLayout` (Update) | `SetWidth`/`SetHeight` inside `View()` | §5.2 |
| Mutate the child directly in the same `Update` | wrapping a sync transition in a `func() tea.Msg{}` Cmd | §5.4 |
| Clamp render output with `MaxWidth`/`MaxHeight` | bare `Height()`/`Width()` (Height pads, Width wraps) | §4.4 |
| Trust a sized child's `View()` | a page re-clamping it with `MaxWidth`/`MaxHeight` | §4.4 |
| Treat lipgloss `Width(n)`/`Height(n)` as border-box (subtract frame once) | subtracting the frame in both `recalcLayout` and `View` | §4.3 |

---

## 11. Pre-Merge Checklist (TUI)

Verify alongside the root checklist (root `CLAUDE.md` §11):

- [ ] `Init`, `Update`, `View` use value receivers.
- [ ] No Model holds `context.Context` or `*log.Logger` (pass into the Cmd instead).
- [ ] Every child `Update` Cmd is accumulated and returned via `tea.Batch`.
- [ ] `tea.WindowSizeMsg` reaches every child that stores dimensions.
- [ ] `View()` is pure — no assignments that outlive it, no I/O, no `SetWidth`/`SetHeight`.
- [ ] Every rendered piece of state lives on the component that renders it (State Residency Rule).
- [ ] Dimensions come from a child's `Height()`/`Width()`, not a package-level `const`.
- [ ] Each key string maps to exactly one binding.
- [ ] Chord/multi-press state lives in the component that renders the feedback.
- [ ] Every `tea.Cmd` closure captures locals, not model fields or method calls.
- [ ] No page holds rendering state that belongs to a child component.
