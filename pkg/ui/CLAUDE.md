# Rune — TUI Architecture (`pkg/ui/`)

This file extends the repository-root `CLAUDE.md` and governs all code under `pkg/ui/` — the only Bubble Tea / Elm-cycle code in the repo. It carries §2–§7 plus the UI-architecture rows of the §9 pitfalls table and §11 pre-merge checklist.

**The root `CLAUDE.md` always loads alongside this file. Its §0 (Prime Directive — protect the user's words) and §1 (Go fundamentals) bind here too — read them first; this file does not repeat them.** References below to §0/§1/§8/§10 point to the root file.

Section numbers match the original monolithic `CLAUDE.md` and are **stable** — source code and ADRs cite them by number (e.g. §3.2, §6.3). Do NOT renumber.

---

## 2. Component Architecture

Most architectural defects stem from violating this section.

### 2.1 The State Residency Rule

> **The component that RENDERS a piece of state MUST OWN that state on its Model.**

Litmus test: if deleting a component's `View()` call from its parent would turn a parent field into dead code, that field belongs on the component, not the parent.

```go
// WRONG — page holds the editor's rendering state
type Model struct {            // pages/workspace
    viewport   viewport.Model  // ← only rendered in the center pane
    openPath   string          // ← only displayed by the editor
    breadcrumb breadcrumb.Model
}

// RIGHT — editor owns its rendering state; page holds it by value, untouched
type Model struct { editor editor.Model }  // pages/workspace
```

### 2.2 Component Contract

Every component MUST expose this contract via concrete methods (NOT Go interfaces — avoid boxing):

```go
func New(keys keymap.Bindings, st styles.Styles) Model // constructor with deps
func (m Model) Init() tea.Cmd
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)
func (m Model) View() string                           // pure render (or tea.View)
func (m Model) SetSize(w, h int) Model                  // accept allocated dimensions
func (m Model) Height() int                             // intrinsic/allocated height
```

- `SetSize` returns a new Model (value semantics); the component stores its dimensions and uses them in `View()`.
- `Height()` returns intrinsic or currently-allocated height. Parents query it — they NEVER hardcode it.
- Components that manage focus MUST expose `SetFocused(bool) Model`.

### 2.3 Pages vs Components

| Concern | Page | Component |
|---------|------|-----------|
| Holds child Models | ✓ by value | ✓ by value (sub-components) |
| Handles focus routing | ✓ | ✗ (receives `SetFocused`) |
| Translates cross-component messages | ✓ | ✗ |
| Owns rendering state (text, content, scroll position) | ✗ NEVER | ✓ ALWAYS |
| Defines layout geometry | ✓ queries children, allocates remaining space | ✗ accepts via `SetSize` |
| Emits I/O commands (file reads, network) | ✓ orchestrates | ✓ for component-owned I/O |
| Handles global keybindings (quit, focus change) | ✓ | ✗ |
| Handles scoped keybindings (navigation, selection) | ✗ forwards to focused child | ✓ when `m.focused` |

**Extraction rule:** if a page's `View()` composes multiple sub-views into a logical unit (e.g. breadcrumb + viewport = "editor pane"), that unit MUST be extracted into a component. A page MUST NOT inline rendering logic for what should be a component.

### 2.4 Message Ownership & Flow

Messages are defined in the **producer's** package — the component that generates the event or owns the data the message carries.

| Message type | Defined in | Consumed by |
|---|---|---|
| `FileSelectedMsg{Path}` | `components/filetree` | page (orchestrates file load) |
| `FileLoadedMsg{Path, Content}` | `components/editor` | `components/editor` (owns viewport content) |
| `TabSelectedMsg{Path}` | `components/opentabs` | page (orchestrates file load) |
| `ConfirmQuitMsg` | `components/footer` | page (calls `tea.Quit`) |

**Rule:** if a message carries data that only ONE component renders, it MUST be defined in that component's package.

**Cmd factory rule:** if a `tea.Cmd` produces a message that only one component consumes, both the factory AND the message type live in that component's package. The page calls the factory — it does not implement the I/O.

```go
// Page receives from child A, translates, forwards to child B
case filetree.FileSelectedMsg:
    return m, editor.LoadFileCmd(msg.Path)   // editor defines + handles its own load
case editor.FileLoadedMsg:                   // editor handled it internally; page forwards
    m.opentabs, cmd = m.opentabs.Update(opentabs.FileOpenedMsg{Path: msg.Path})
```

**Internal messages:** timer/expiry messages for state machines (e.g. chord timeout) MUST be unexported types within the owning component. Only the final result message (e.g. `ConfirmQuitMsg`) is exported.

### 2.5 Dependency Injection

- Pass `keymap.Bindings` and `styles.Styles` as value types via constructors.
- NEVER store `context.Context` (see §6) or `*log.Logger` as a Model field. If a `tea.Cmd` needs a logger, pass it into the Cmd factory.
- Shared UI environment (styles, keymaps) flows top-down through constructors. Components do NOT import pages or domain packages.

---

## 3. Keybinding Architecture

### 3.1 Binding Organization

All keybindings live in `pkg/ui/keymap/keymap.go` as a single `Bindings` struct — the single source of truth.

**No overlapping bindings:** before adding a key, verify NO other binding uses that key string. If `ctrl+1` is in `TabSwitch`, it MUST NOT also be in `FocusLeft`. One physical keypress MUST resolve to exactly ONE logical action. Scan every `key.WithKeys(...)` in `Default()`; each key string appears in exactly one binding — duplicates are a compile-time-equivalent defect.

### 3.2 Chord & Multi-Press Keybindings

A chord is a multi-press sequence (e.g. `^C` twice to quit). The chord state machine MUST be owned by the component that **renders the confirmation feedback**.

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
// The page only handles the result:  case footer.ConfirmQuitMsg: return m, tea.Quit
```

**NEVER implement chord state in a page** — it does not render the prompt, so it must not own the pending state. A page MUST NOT match chord-sequence bindings (`ConfirmExitC`, etc.) in its own `KeyPressMsg` switch; it forwards the `KeyPressMsg` to the owning component, which matches internally and emits the result message when the chord completes.

### 3.3 Focus-Scoped Key Handling

Handle global keys FIRST in the page; then forward the message to all children, which internally gate on `m.focused`.

```go
// Page Update
case tea.KeyPressMsg:
    switch { // global keys — handled regardless of focus
    case key.Matches(msg, m.keys.FocusExplorer): m.focus = paneTree
    case key.Matches(msg, m.keys.ZenMode):       m.leftVisible = !m.leftVisible
    }
    // do NOT forward global keys to children (return early / use a consumed flag)
// then forward, letting children gate on focus:
//   m.filetree = m.filetree.SetFocused(m.focus == paneTree)
//   m.filetree, cmd = m.filetree.Update(msg)

// Inside a component
case tea.KeyPressMsg:
    if !m.focused { return m, nil } // ignore keys when not focused
    // handle navigation keys...
```

### 3.4 Key Matching Order

`key.Matches` checks run sequentially, so order matters:

1. **Chord completions** — check pending state first (most specific).
2. **Global actions** — quit, focus changes, zen mode.
3. **Contextual actions** — tab switch, pin, close (state-dependent).
4. **Fallthrough to children** — navigation, selection.

---

## 4. Layout & Dimensions

### 4.1 Intrinsic vs Extrinsic Sizing

- **Intrinsic:** a component knows its own natural size (footer = 1 line; opentabs = `len(tabs)+1`). Exposed via `Height() int`.
- **Extrinsic:** a parent allocates space (available room minus children's intrinsic sizes), passed via `SetSize(w, h)`.

NEVER use a package-level `const` for a dimension that belongs to a component — query the component instead:

```go
const footerH = 1; contentH := totalH - footerH  // WRONG — magic number
contentH := totalH - m.footer.Height()            // RIGHT — query the component
const leftPaneW = 22                              // WRONG for a child's width too
leftW := m.sidebar.PreferredWidth()               // RIGHT — a method, or a configurable field
```

### 4.2 Dimension Calculation Flow

All dimension math happens in a `recalcLayout()` method called from `Update`, after `tea.WindowSizeMsg` OR any structural change (pane toggled, tab added/removed): query children's intrinsic sizes → compute allocations → `SetSize(w,h)` every child → return the model.

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

In lipgloss v2, `Width(n)`/`Height(n)` are **border-box**: `n` is the **total** rendered dimension including borders and padding; the content area is `n - frame_size`. `Height(n)` is minimum-only — it pads short content but NEVER truncates tall content.

**Subtract the frame ONCE — in `recalcLayout()`, when computing child dimensions.** The border style in `View()` receives the OUTER dimension; children receive INNER (outer − frame). Do NOT subtract in both places.

```go
// recalcLayout — inner dimension for the child (subtract the frame ONCE)
innerH := contentH - 2
m.editor = m.editor.SetSize(innerCenterW, innerH)
// View — the border style gets the OUTER dimension
borderStyle.Width(centerW).Height(contentH).Render(m.editor.View())
// NOT Width(centerW-2).Height(contentH-2) — that double-subtracts.
```

Use `GetHorizontalFrameSize()`/`GetVerticalFrameSize()` instead of hardcoding `2` when you need the exact overhead.

### 4.4 Hard Dimension Clamping (`MaxWidth` / `MaxHeight`)

- `Height(n)` is a **minimum** (pads, never truncates); `Width(n)` **wraps** overflow beyond `n - frame_size` cells (ADDING visual lines).
- `MaxHeight(n)` / `MaxWidth(n)` **truncate** — hard-clamp total output to at most `n` lines / each line to at most `n` cells (no wrapping).

**Clamping is single-owner.** The component that receives `SetSize(w,h)` is solely responsible for making its `View()` fit `w×h`, via `MaxWidth(m.width).MaxHeight(m.height)` as the outermost style in `View()`:

```go
func (m Model) View() string {
    content := /* ... render sub-views ... */
    return lipgloss.NewStyle().MaxWidth(m.width).MaxHeight(m.height).Render(content)
}
```

**Pages MUST NOT re-clamp child output.** Once a child is sized, the page trusts its `View()` contract. Re-wrapping a child view in `MaxWidth`/`MaxHeight` is redundant if the child is correct, masks the bug if it isn't, and duplicates dimension math.

```go
borderStyle.Render(m.editor.View())  // RIGHT — trust the child contract
// WRONG: borderStyle.Render(lipgloss.NewStyle().MaxWidth(w).MaxHeight(h).Render(m.editor.View()))
```

Pages DO use `Height`/`Width` on their own layout wrappers (e.g. border styles) to fill space — that is the page's own box-sizing, not child-clamping. The border's `Height(innerH)` pads the box if the child's content is shorter.

### 4.5 Rendering & Unicode

Every rendering calculation — span position, cell column, wrap boundary, table cell width — counts **display characters**, not bytes (see §1.5 in the root file).

```go
for i := 0; i < len(sp.Text); i++ { /* WRONG — byte offsets break on CJK/emoji/accents */ }
cellEnd := cellStart + utf8.RuneCountInString(cellText) // RIGHT — rune-based offsets
```

If the calculation affects what the user sees, use `utf8.RuneCountInString`. Purely internal offsets (buffer, syntax tree) may use `len()` — but double-check that assumption.

---

## 5. The Elm Cycle

### 5.1 Value Receivers — No Exceptions

`Init`, `Update`, `View` MUST use value receivers — `func (m Model) Update(...)`, NEVER `func (m *Model) Update(...)`. Do NOT add pointer receivers to satisfy an interface; if a bubbles library requires the pointer-based `tea.Model`, wrap it with a thin adapter at the boundary — do not infect the component tree.

### 5.2 Pure View

`View()` MUST be a pure function of Model state. It MUST NOT: assign to variables that outlive the call; perform I/O; call `SetWidth`/`SetHeight` or any mutating method on child models; return or emit a `tea.Cmd`.

### 5.3 Non-Blocking Update

NEVER block in `Update()` or `Init()`. File I/O, network, and timers each become a `tea.Cmd` that returns a typed result `Msg` (a timer is a Cmd with `time.Sleep` inside that returns an expiry `Msg`). `time.Sleep` directly in `Update` is ALWAYS wrong.

### 5.4 Synchronous State vs Async Commands

`tea.Cmd` is ONLY for operations that leave the goroutine: file I/O, network, timers, syscalls. NEVER wrap a synchronous state transition in a Cmd:

```go
// WRONG — wrapping a state change in a Cmd to "send a message to self"
cmds = append(cmds, func() tea.Msg { return opentabs.TabSwitchIndexMsg{Index: idx} })
// RIGHT — mutate the child directly in the same Update pass; only the I/O is a Cmd
if path := m.opentabs.PathAt(idx); path != "" {
    m.opentabs = m.opentabs.SelectIndex(idx)
    cmds = append(cmds, editor.LoadFileCmd(path))
}
```

A self-messaging Cmd delays delivery by one frame, creates page→Cmd→runtime→page→child ping-pong, and leaks internal message types. **If the page already knows the intent and has the data, it MUST act directly via child methods.** Components expose query + mutation methods for the page to orchestrate:

```go
func (m Model) PathAt(i int) string      // query: what file is at this index?
func (m Model) SelectIndex(i int) Model  // mutate: switch active tab
func (m Model) PinIndex(i int) Model     // mutate: toggle pin
func (m Model) OpenFile(p string) Model  // mutate: add/activate tab
func (m Model) CloseFile(p string) Model // mutate: remove tab
```

Components emit messages ONLY to report user-initiated actions the page cannot anticipate (e.g. Enter on a focused item):

```go
case key.Matches(msg, m.keys.Select):
    path := m.tabs[m.cursor].Path
    return m, func() tea.Msg { return TabSelectedMsg{Path: path} }
```

### 5.5 Cmd Closure Safety

ALWAYS capture model-derived values into locals BEFORE the closure — it runs asynchronously, so the snapshot at creation is what matters (true even with value receivers):

```go
// WRONG — reads a model field at execution time (race: m may change)
cmds = append(cmds, func() tea.Msg { return TabPinnedMsg{Index: m.opentabs.Cursor()} })
// RIGHT — capture into a local first
idx := m.opentabs.Cursor()
cmds = append(cmds, func() tea.Msg { return TabPinnedMsg{Index: idx} })
```

### 5.6 Cmd Forwarding & Batching

After every `child.Update(msg)`, capture and accumulate the returned `tea.Cmd` — NEVER drop it by ignoring the second return. Return `tea.Batch(cmds...)`. `tea.WindowSizeMsg` MUST reach every child that stores dimensions.

```go
m.editor, cmd = m.editor.Update(msg); cmds = append(cmds, cmd) // NEVER omit this line
m.footer, cmd = m.footer.Update(msg); cmds = append(cmds, cmd)
return m, tea.Batch(cmds...)
```

### 5.7 Viewport Scroll Preservation

`viewport.SetContent()` is scroll-destructive. Preserve the offset around it:

```go
wasAtBottom := m.viewport.AtBottom()
offset := m.viewport.YOffset
m.viewport.SetContent(newContent)
if wasAtBottom { m.viewport.GotoBottom() } else { m.viewport.SetYOffset(offset) }
```

---

## 6. Context & Async Operations

### 6.1 Context on Models — PROHIBITED

NEVER store `context.Context` as a struct field on any Model (or any "shared"/`Common` struct):

```go
type Model struct { ctx context.Context } // ✗ NEVER — even in a shared struct
```

### 6.2 Context in Cmd Closures — ENCOURAGED

Pass context INTO `tea.Cmd` factory functions for cancellation of I/O:

```go
func loadFileCmd(ctx context.Context, path string) tea.Cmd {
    return func() tea.Msg {
        select {
        case <-ctx.Done(): return FileLoadCancelledMsg{Path: path}
        default:
        }
        b, err := os.ReadFile(path)
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

`pkg/ui/app.go`'s Model is a **router**: it holds zero rendering state and delegates entirely to the active page.

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
- Backgrounded pages retain their state but do NOT receive messages.
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

UI-layer mistakes LLMs commonly produce here; each is a hard violation. (Data-safety and Go pitfalls are in the root `CLAUDE.md` §9.)

| # | Anti-Pattern | Why It's Wrong | Rule Violated |
|---|---|---|---|
| 1 | Defining a Go `interface` for a single concrete component | Unnecessary boxing. Components are concrete value types. | §2.2 |
| 2 | Adding pointer receivers to `Init`/`Update`/`View` | Breaks value semantics, enables aliasing | §5.1 |
| 3 | Storing `context.Context` or `*log.Logger` on Model | Violates Elm purity, leaks non-serializable state | §6.1 |
| 4 | Defining a message in the CONSUMER's package | Message belongs to the producer/owner of the data | §2.4 |
| 5 | Page holds state that only one child component renders | Violates State Residency Rule | §2.1 |
| 6 | Package-level `const` for a child component's dimension | Magic number; component must expose `Height()`/`Width()` | §4.1 |
| 7 | Same key string appears in two different Bindings fields | Keybinding collision — ambiguous dispatch | §3.1 |
| 8 | Capturing `m.field` or `m.child.Method()` inside a `tea.Cmd` closure | Race condition; must capture into local variable | §5.4 |
| 9 | Implementing chord/confirm state in a page instead of the rendering component | Chord state machine belongs to the component that displays feedback | §3.2 |
| 10 | Returning `nil` Cmd without propagating child Cmds | Silently dropped commands | §5.5 |
| 11 | Forwarding `tea.KeyPressMsg` to ALL children unconditionally | Unfocused components must not process navigation keys | §3.3 |
| 12 | Using `time.Sleep` inside `Update()` or `Init()` | Blocks the event loop; must use a Cmd | §5.3 |
| 13 | Inlining a multi-view logical unit in a page's `View()` instead of extracting a component | God-struct; violates extraction rule | §2.3 |
| 14 | Adding a `SetWidth`/`SetHeight` call inside `View()` | Mutates state in a pure function | §5.2 |
| 15 | Wrapping synchronous state transitions in `func() tea.Msg{}` Cmds | Unnecessary frame delay, message ping-pong, leaks internals | §5.4 |
| 16 | Using lipgloss `Height()`/`Width()` without `MaxHeight()`/`MaxWidth()` at render boundaries | Content overflows: Height is minimum-only, Width wraps (adding lines) | §4.4 |
| 17 | Page re-clamping child output with `MaxWidth`/`MaxHeight` wrappers | Redundant, masks bugs, duplicates dimension arithmetic | §4.4 |
| 18 | Treating lipgloss `Width(n)`/`Height(n)` as content-box (subtracting frame in BOTH recalcLayout and View) | Double-subtraction: Width/Height are border-box in lipgloss v2 | §4.3 |

---

## 11. Pre-Merge Checklist (TUI)

Verify alongside the root checklist (root `CLAUDE.md` §11):

- [ ] No pointer receivers on `Init`, `Update`, `View` methods.
- [ ] No `context.Context` or `*log.Logger` fields on any Model struct.
- [ ] ALL `tea.Cmd` from child `.Update()` calls are accumulated and returned via `tea.Batch`.
- [ ] `tea.WindowSizeMsg` reaches every child that stores dimensions.
- [ ] `View()` is pure — no assignments, no I/O, no `SetWidth`/`SetHeight`.
- [ ] Every rendered piece of state lives on the component that renders it (State Residency Rule).
- [ ] No package-level `const` for dimensions — components expose `Height()`/`Width()`.
- [ ] No overlapping key strings across different `Bindings` fields.
- [ ] Chord/multi-press state lives in the component that renders the feedback.
- [ ] All `tea.Cmd` closures capture local variables, not model fields or method calls.
- [ ] No page holds rendering state that belongs to a child component.
