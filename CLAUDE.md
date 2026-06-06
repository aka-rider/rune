# Rune — Engineering Standard & Code Convention

These instructions are the **single source of truth** for all code produced in this repository.
Every rule uses imperative language. "MUST" means mandatory. "NEVER" means prohibited.
If code violates any rule below, it is defective and must be fixed before merge.

`rune` is a Bubble Tea (v2) TUI markdown editor and note-taking app built in Go.

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
- **NEVER swallow errors.** Bare `_, _ = operation()` is prohibited unless annotated with `// fire-and-forget: <reason>`.
- **Contextual wrapping.** Always wrap errors with operation + resource context: `fmt.Errorf("load dir %q: %w", dir, err)`.
- **No silent fallbacks.** Do not default or silently recover from invalid user-supplied data.

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

## 2. Component Architecture

This is the most critical section. Most architectural defects stem from violating these rules.

### 2.1 The State Residency Rule

> **The component that RENDERS a piece of state MUST OWN that state on its Model.**

Litmus test: If you delete a component's `View()` call from its parent, would a field on the parent become dead code? That field belongs on the component, not the parent.

**WRONG — page holds editor state:**
```go
// pages/workspace/workspace.go
type Model struct {
    viewport   viewport.Model  // ← only rendered in center pane
    openPath   string          // ← only displayed by breadcrumb/viewport
    breadcrumb breadcrumb.Model
    // ...
}
```

**RIGHT — editor component owns its rendering state:**
```go
// components/editor/editor.go
type Model struct {
    viewport   viewport.Model
    breadcrumb breadcrumb.Model
    openPath   string
    // ...
}

// pages/workspace/workspace.go
type Model struct {
    editor   editor.Model  // ← page holds child by value, does not touch internals
    // ...
}
```

### 2.2 Component Contract

Every component MUST expose this interface via concrete methods (NOT via Go interfaces — avoid boxing):

```go
func New(keys keymap.Bindings, st styles.Styles) Model  // Constructor with dependencies
func (m Model) Init() tea.Cmd                           // Startup commands
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)     // State transitions
func (m Model) View() string                            // Pure render (or tea.View)
func (m Model) SetSize(w, h int) Model                  // Accept allocated dimensions
func (m Model) Height() int                             // Intrinsic/allocated height
```

- `SetSize` returns a new Model (value semantics). The component stores its allocated dimensions and uses them in `View()`.
- `Height()` returns the component's intrinsic or currently-allocated height. Parents query this — they NEVER hardcode it.
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

**Extraction rule:** If a page's `View()` composes multiple sub-views into a logical unit (e.g., breadcrumb + viewport = "editor pane"), that unit MUST be extracted into a component. A page MUST NOT inline rendering logic for what should be a component.

### 2.4 Message Ownership & Flow

Messages are defined in the **producer's** package — the component that generates the event or owns the data the message carries.

| Message type | Defined in | Consumed by |
|---|---|---|
| `FileSelectedMsg{Path}` | `components/filetree` | page (orchestrates file load) |
| `FileLoadedMsg{Path, Content}` | `components/editor` | `components/editor` (owns viewport content) |
| `TabSelectedMsg{Path}` | `components/opentabs` | page (orchestrates file load) |
| `ConfirmQuitMsg` | `components/footer` | page (calls `tea.Quit`) |

**Rule:** If a message carries data that only ONE component renders, that message MUST be defined in that component's package.

**Cmd factory rule:** If a `tea.Cmd` produces a message that only one component consumes, both the Cmd factory AND the message type MUST live in that component's package. The page calls the factory — it does not implement the I/O itself.

**Cross-component coordination pattern:**
```go
// Page receives from child A, translates, forwards to child B
case filetree.FileSelectedMsg:
    return m, editor.LoadFileCmd(msg.Path)  // editor defines and handles its own load

case editor.FileLoadedMsg:
    // editor already handled this internally; page just forwards to opentabs
    m.opentabs, cmd = m.opentabs.Update(opentabs.FileOpenedMsg{Path: msg.Path})
```

**Internal messages:** Timer/expiry messages for state machines (e.g., chord timeout) MUST be unexported types within the owning component. Only the final result message (e.g., `ConfirmQuitMsg`) is exported.

### 2.5 Dependency Injection

- Pass `keymap.Bindings` and `styles.Styles` as value types via constructors.
- NEVER store `context.Context` as a Model field (see §6).
- NEVER store `*log.Logger` as a Model field. If logging is needed in a `tea.Cmd`, pass the logger into the Cmd factory function.
- Shared UI environment (styles, keymaps) flows top-down through constructors. Components do NOT import pages or domain packages.

---

## 3. Keybinding Architecture

### 3.1 Binding Organization

All keybindings live in `pkg/ui/keymap/keymap.go` as a single `Bindings` struct. This is the single source of truth.

**No overlapping bindings rule:** Before adding a key to a binding, verify NO other binding in the struct already uses that key string. If `ctrl+1` appears in `TabSwitch` keys, it MUST NOT also appear in `FocusLeft` keys. A single physical keypress MUST resolve to exactly ONE logical action.

**Verification:** Scan all `key.WithKeys(...)` calls in the `Default()` function. Every key string MUST appear in exactly one binding. Duplicate key strings across different bindings are a compile-time-equivalent defect.

### 3.2 Chord & Multi-Press Keybindings

A chord is a multi-press sequence (e.g., press `^C` twice to quit). The state machine for a chord MUST be owned by the component that **renders the confirmation feedback**.

**Pattern:**
```go
// components/footer/footer.go — owns the confirm-exit state machine

type ConfirmQuitMsg struct{}  // emitted when chord completes
type confirmExpired struct{}  // internal, unexported

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
    switch msg := msg.(type) {
    case tea.KeyPressMsg:
        switch {
        case key.Matches(msg, m.keys.ConfirmExitC):
            if m.pendingKey == "c" {
                m.pendingKey = ""
                return m, func() tea.Msg { return ConfirmQuitMsg{} }
            }
            m.pendingKey = "c"
            return m, m.startConfirmTimer()
        }
    case confirmExpired:
        m.pendingKey = ""
    }
    return m, nil
}

func (m Model) startConfirmTimer() tea.Cmd {
    return func() tea.Msg {
        time.Sleep(2 * time.Second)
        return confirmExpired{}
    }
}

// View shows "Press ^C again to exit" when m.pendingKey == "c"
```

**The page only handles the final result:**
```go
// pages/workspace/workspace.go
case footer.ConfirmQuitMsg:
    return m, tea.Quit
```

**NEVER implement chord state in a page.** The page does not render the confirmation prompt, so it must not own the pending state.

**Chord key routing rule:** A page MUST NOT match against chord-sequence key bindings (e.g., `ConfirmExitC`, `ConfirmExitD`) in its own `tea.KeyPressMsg` switch. Instead, the page forwards the `tea.KeyPressMsg` to the component that owns the chord (e.g., footer). The component handles the binding match internally and emits the result message when the chord completes.

### 3.3 Focus-Scoped Key Handling

```go
// Page Update — handle global keys FIRST, then forward to focused child
case tea.KeyPressMsg:
    // Global keys — always handled regardless of focus
    switch {
    case key.Matches(msg, m.keys.FocusExplorer):
        m.focus = paneTree
    case key.Matches(msg, m.keys.ZenMode):
        m.leftVisible = !m.leftVisible
    }
    // DO NOT forward global keys to children — return early or use a consumed flag

// After the switch, forward msg to ALL children, but children internally gate on m.focused:
m.filetree = m.filetree.SetFocused(m.focus == paneTree)
m.filetree, cmd = m.filetree.Update(msg)
```

**Inside a component:**
```go
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
    switch msg := msg.(type) {
    case tea.KeyPressMsg:
        if !m.focused {
            return m, nil  // ignore keys when not focused
        }
        // handle navigation keys...
    }
}
```

### 3.4 Key Matching Order

When a `tea.KeyPressMsg` could match multiple bindings (because `key.Matches` checks are sequential), order matters:

1. **Chord completions** — check pending state first (most specific)
2. **Global actions** — quit, focus changes, zen mode
3. **Contextual actions** — tab switch, pin, close (depend on current state)
4. **Fallthrough to children** — navigation, selection

---

## 4. Layout & Dimensions

### 4.1 Intrinsic vs Extrinsic Sizing

- **Intrinsic:** A component knows its own natural height. Footer is always 1 line. OpenTabs is `len(tabs) + 1`. These are exposed via `Height() int`.
- **Extrinsic:** A parent allocates space to a child based on available room minus other children's intrinsic sizes. This allocated size is passed via `SetSize(w, h)`.

**NEVER use package-level `const` for a dimension that belongs to a component:**
```go
// WRONG — magic number at page level
const footerH = 1
contentH := totalH - footerH

// RIGHT — query the component
contentH := totalH - m.footer.Height()
```

**NEVER use package-level `const` for a child's preferred width:**
```go
// WRONG
const leftPaneW = 22

// RIGHT — component exposes a method, or make it a configurable Model field
leftW := m.sidebar.PreferredWidth()
// OR: store as a field that can be changed (e.g., user dragged a divider)
```

### 4.2 Dimension Calculation Flow

All dimension math MUST happen in a `recalcLayout()` method called from `Update`:
1. Triggered after `tea.WindowSizeMsg` OR any structural change (pane toggled, tab added/removed).
2. Queries intrinsic sizes from children.
3. Computes allocated sizes for flexible children.
4. Calls `SetSize(w, h)` on every child with their allocated dimensions.
5. Returns the updated Model.

```go
func (m Model) recalcLayout() Model {
    contentH := m.totalHeight - m.footer.Height()
    // ... compute leftW, centerW based on visibility and intrinsic widths
    m.editor = m.editor.SetSize(centerW, contentH)
    m.filetree = m.filetree.SetSize(leftW, contentH)
    m.footer = m.footer.SetSize(m.totalWidth, m.footer.Height())
    return m
}
```

### 4.3 Border Arithmetic — lipgloss v2 is Border-Box

**Critical:** In lipgloss v2, `Width(n)` and `Height(n)` use **border-box** semantics. The value `n` is the **total rendered dimension** including borders and padding. The content area is `n - frame_size`.

```go
// Width(20) on a bordered style:
//   Total rendered width = 20
//   Content area = 20 - GetHorizontalFrameSize() = 20 - 2 = 18
//   Text wraps at 18 cells

// Height(30) on a bordered style:
//   Minimum total rendered height = 30
//   Content area = 30 - GetVerticalFrameSize() = 30 - 2 = 28
//   Content shorter than 28 lines is padded
//   Content TALLER than 28 lines is NOT truncated (Height is still minimum-only)
```

**Subtract border size ONCE — in `recalcLayout()` when computing child dimensions.** The border style in `View()` receives the OUTER dimension (the full box size). Children receive the INNER dimension (outer minus frame). Do NOT subtract the frame in both places.

```go
// recalcLayout — compute inner dimensions for children
innerH := contentH - 2  // subtract border ONCE here
m.editor = m.editor.SetSize(innerCenterW, innerH)

// View — pass OUTER dimensions to border style
borderStyle.Width(centerW).Height(contentH).Render(m.editor.View())
// NOT: Width(centerW - 2).Height(contentH - 2) — that double-subtracts!
```

Use `GetHorizontalFrameSize()` / `GetVerticalFrameSize()` when you need the exact frame overhead programmatically rather than hardcoding `2`.

### 4.4 Hard Dimension Clamping (`MaxWidth` / `MaxHeight`)

In lipgloss:
- `Height(n)` is a **minimum** — it pads short content but NEVER truncates tall content.
- `Width(n)` **wraps** content that exceeds `n - frame_size` cells (which ADDS visual lines).
- `MaxHeight(n)` **truncates** — hard clamp total output to at most `n` lines.
- `MaxWidth(n)` **truncates** — hard clamp each line to at most `n` cells (no wrapping).

**Clamping responsibility is single-owner.** The component that receives allocated dimensions via `SetSize(w, h)` is solely responsible for ensuring its `View()` output fits within `w × h`. It does this with `MaxWidth(m.width).MaxHeight(m.height)` as the outermost render style in `View()`.

```go
// Component's View() — guarantees output fits allocated dimensions
func (m Model) View() string {
    content := /* ... render sub-views ... */
    return lipgloss.NewStyle().
        MaxWidth(m.width).
        MaxHeight(m.height).
        Render(content)
}
```

**Pages MUST NOT re-clamp child output.** Once a child receives dimensions via `SetSize`, the page trusts the child's `View()` contract. Adding `MaxHeight`/`MaxWidth` wrappers around child views in the page is:
- Redundant if the child is correct (dead code that obscures intent)
- A bug-mask if the child is broken (hides overflow instead of surfacing it)
- A maintenance hazard (dimension arithmetic duplicated in two places)

```go
// WRONG — page re-clamps child output it already sized
clampedEditor := lipgloss.NewStyle().MaxWidth(w).MaxHeight(h).Render(m.editor.View())
borderStyle.Render(clampedEditor)

// RIGHT — page trusts the component contract
borderStyle.Render(m.editor.View())
```

**Pages use `Height`/`Width` on their own layout wrappers** (e.g., border styles) to fill available space — this is the page's own box-sizing concern, not a child-clamping concern. The child's output fits because the child guarantees it; the border's `Height(innerH)` pads the box if content is shorter.

### 4.5 Rendering & Unicode

Every rendering calculation — span position, cell column, wrap boundary, table cell width — must count **display characters**, not bytes.

```go
// WRONG — byte offsets break on CJK, emoji, accented chars
for i := 0; i < len(sp.Text); i++ { /* ... */ }
cellEnd := cellStart + len(cellText)

// RIGHT — rune-based offsets
for i := 0; i < utf8.RuneCountInString(sp.Text); i++ { /* ... */ }
cellEnd := cellStart + utf8.RuneCountInString(cellText)
```

**Rule:** If the calculation affects what the user sees on screen, use `utf8.RuneCountInString`. If the calculation is purely internal (buffer offsets, syntax tree positions), `len()` is fine — but double-check that assumption.

---

## 5. The Elm Cycle

### 5.1 Value Receivers — No Exceptions

`Init`, `Update`, and `View` MUST use value receivers:
```go
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)  // ✓
func (m *Model) Update(msg tea.Msg) (Model, tea.Cmd) // ✗ NEVER
```

Do NOT add pointer receivers to satisfy interfaces. If a bubbles library requires `tea.Model` interface (pointer-based), wrap with a thin adapter at the boundary — do not infect the entire component tree.

### 5.2 Pure View

`View()` MUST be a pure function of Model state. It MUST NOT:
- Assign to variables that outlive the call
- Perform I/O
- Call `SetWidth()`, `SetHeight()`, or any mutating method on child models
- Return or emit `tea.Cmd`

### 5.3 Non-Blocking Update

NEVER perform blocking operations in `Update()` or `Init()`:
- File I/O → `tea.Cmd` that returns a `FileLoadedMsg`
- Network calls → `tea.Cmd` that returns a result `Msg`
- Timers → `tea.Cmd` with `time.Sleep` inside, returns a typed expiry `Msg`
- `time.Sleep` directly in `Update` is ALWAYS wrong

### 5.4 Synchronous State vs Async Commands

`tea.Cmd` is ONLY for operations that require leaving the current goroutine: file I/O, network, timers, system calls. NEVER wrap a synchronous state transition in a Cmd:

```go
// WRONG — wrapping a state change in a Cmd to "send a message to self"
cmds = append(cmds, func() tea.Msg { return opentabs.TabSwitchIndexMsg{Index: idx} })

// RIGHT — mutate the child directly in the same Update pass
if path := m.opentabs.PathAt(idx); path != "" {
    m.opentabs = m.opentabs.SelectIndex(idx)
    cmds = append(cmds, editor.LoadFileCmd(path))  // only the I/O is a Cmd
}
```

**Why this matters:**
- A `func() tea.Msg { return X{} }` Cmd delays delivery by one frame (async execution).
- It creates message ping-pong: page → Cmd → runtime → page → child — when direct mutation achieves the same in one pass.
- It leaks component internals: the page must define intermediate message types that should not exist.

**Rule:** If the page already knows the intent and has enough information to act, it MUST act directly via method calls on child models. Only emit a `tea.Cmd` when actual I/O or a timer is required.

**Components expose query + mutation methods for the page to orchestrate:**
```go
// components/opentabs — query methods for page use
func (m Model) PathAt(index int) string   // query: what file is at this index?
func (m Model) SelectIndex(int) Model     // mutate: switch active tab
func (m Model) PinIndex(int) Model        // mutate: toggle pin
func (m Model) OpenFile(path string) Model // mutate: add/activate tab
func (m Model) CloseFile(path string) Model // mutate: remove tab
```

**Components emit messages ONLY to report user-initiated actions** that the page cannot anticipate (e.g., user pressing Enter on a focused list item):
```go
// opentabs emits TabSelectedMsg only when user navigates within the focused component
case key.Matches(msg, m.keys.Select):
    path := m.tabs[m.cursor].Path
    return m, func() tea.Msg { return TabSelectedMsg{Path: path} }
```

### 5.5 Cmd Closure Safety

**ALWAYS capture model-derived values into local variables before the closure:**

```go
// WRONG — captures model field that may be stale
cmds = append(cmds, func() tea.Msg {
    return TabPinnedMsg{Index: m.opentabs.Cursor()}  // ← race: m may change
})

// RIGHT — capture into local
idx := m.opentabs.Cursor()
cmds = append(cmds, func() tea.Msg {
    return TabPinnedMsg{Index: idx}
})
```

This applies even with value receivers. The closure executes asynchronously — the model snapshot at closure creation is what matters.

### 5.6 Cmd Forwarding & Batching

- After calling `child.Update(msg)`, ALWAYS capture and accumulate the returned `tea.Cmd`.
- NEVER silently drop a `Cmd` by ignoring the second return value.
- At the end of `Update`, return `tea.Batch(cmds...)`.
- `tea.WindowSizeMsg` MUST reach every child that stores dimensions.

```go
m.editor, cmd = m.editor.Update(msg)
cmds = append(cmds, cmd)  // NEVER omit this line

m.footer, cmd = m.footer.Update(msg)
cmds = append(cmds, cmd)

return m, tea.Batch(cmds...)
```

### 5.7 Viewport Scroll Preservation

`viewport.SetContent()` is scroll-destructive. Before setting new content:
```go
wasAtBottom := m.viewport.AtBottom()
offset := m.viewport.YOffset
m.viewport.SetContent(newContent)
if wasAtBottom {
    m.viewport.GotoBottom()
} else {
    m.viewport.SetYOffset(offset)
}
```

---

## 6. Context & Async Operations

### 6.1 Context on Models — PROHIBITED

NEVER store `context.Context` as a struct field on any Model:
```go
// WRONG
type Model struct {
    ctx context.Context  // ✗
}

// WRONG
type Common struct {
    ctx context.Context  // ✗ even in a "shared" struct
}
```

### 6.2 Context in Cmd Closures — ENCOURAGED

Pass context INTO `tea.Cmd` factory functions for cancellation of I/O:
```go
func loadFileCmd(ctx context.Context, path string) tea.Cmd {
    return func() tea.Msg {
        select {
        case <-ctx.Done():
            return FileLoadCancelledMsg{Path: path}
        default:
        }
        b, err := os.ReadFile(path)
        if err != nil {
            return ErrMsg{Err: fmt.Errorf("open %q: %w", path, err)}
        }
        return FileLoadedMsg{Path: path, Content: string(b)}
    }
}
```

### 6.3 Cancellation Pattern

For operations that need cancellation (e.g., file watchers, long searches):
```go
type Model struct {
    cancelSearch context.CancelFunc  // stored to cancel previous operation
}

func (m Model) startSearch(query string) (Model, tea.Cmd) {
    if m.cancelSearch != nil {
        m.cancelSearch()  // cancel previous
    }
    ctx, cancel := context.WithCancel(context.Background())
    m.cancelSearch = cancel
    return m, searchCmd(ctx, query)
}
```

---

## 7. Page Transitions & Routing

### 7.1 Top-Level Router

The `pkg/ui/app.go` Model is a **router**. It holds zero rendering state. It delegates entirely to the active page:

```go
type Model struct {
    activePage page  // enum or int index
    workspace  workspace.Model
    settings   settings.Model
}

func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
    switch msg := msg.(type) {
    case NavigateMsg:
        m.activePage = msg.Page
        return m, m.activeModel().Init()
    }
    // delegate to active page
    // ...
}
```

### 7.2 Page Lifecycle

- A page's `Init()` is called when it becomes active (not at app startup for all pages).
- Pages that are backgrounded retain their state but do NOT receive messages.
- Only the active page's `View()` is called.

### 7.3 Inter-Page Communication

Pages communicate via messages routed through the top-level app:
```go
// Page emits a navigation request
case editor.OpenSettingsMsg:
    m.activePage = pageSettings
    return m, m.settings.Init()
```

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

## 9. LLM-Specific Pitfalls

These are mistakes that language models commonly produce in this codebase. Each is a hard violation.

| # | Anti-Pattern | Why It's Wrong | Rule Violated |
|---|---|---|---|
| 1 | Defining a Go `interface` for a single concrete component | Unnecessary boxing. Components are concrete value types. | §2.2 |
| 2 | Adding pointer receivers to `Init`/`Update`/`View` | Breaks value semantics, enables aliasing | §5.1 |
| 3 | Storing `context.Context` or `*log.Logger` on Model | Violates Elm purity, leaks non-serializable state | §6.1 |
| 4 | Creating `types`, `utils`, `helpers`, `common` packages | Catch-all packages with no cohesion | §1.4 |
| 5 | Defining a message in the CONSUMER's package | Message belongs to the producer/owner of the data | §2.4 |
| 6 | Page holds state that only one child component renders | Violates State Residency Rule | §2.1 |
| 7 | Package-level `const` for a child component's dimension | Magic number; component must expose `Height()`/`Width()` | §4.1 |
| 8 | Same key string appears in two different Bindings fields | Keybinding collision — ambiguous dispatch | §3.1 |
| 9 | Capturing `m.field` or `m.child.Method()` inside a `tea.Cmd` closure | Race condition; must capture into local variable | §5.4 |
| 10 | Implementing chord/confirm state in a page instead of the rendering component | Chord state machine belongs to the component that displays feedback | §3.2 |
| 11 | Returning `nil` Cmd without propagating child Cmds | Silently dropped commands | §5.5 |
| 12 | Forwarding `tea.KeyPressMsg` to ALL children unconditionally | Unfocused components must not process navigation keys | §3.3 |
| 13 | Using `time.Sleep` inside `Update()` or `Init()` | Blocks the event loop; must use a Cmd | §5.3 |
| 14 | Inlining a multi-view logical unit in a page's `View()` instead of extracting a component | God-struct; violates extraction rule | §2.3 |
| 15 | Adding a `SetWidth`/`SetHeight` call inside `View()` | Mutates state in a pure function | §5.2 |
| 16 | Wrapping synchronous state transitions in `func() tea.Msg{}` Cmds | Unnecessary frame delay, message ping-pong, leaks internals | §5.4 |
| 17 | Using lipgloss `Height()`/`Width()` without `MaxHeight()`/`MaxWidth()` at render boundaries | Content overflows: Height is minimum-only, Width wraps (adding lines) | §4.4 |
| 18 | Page re-clamping child output with `MaxWidth`/`MaxHeight` wrappers | Redundant, masks bugs, duplicates dimension arithmetic | §4.4 |
| 19 | Treating lipgloss `Width(n)`/`Height(n)` as content-box (subtracting frame in BOTH recalcLayout and View) | Double-subtraction: Width/Height are border-box in lipgloss v2 | §4.3 |
| 20 | Using `len(str)` instead of `utf8.RuneCountInString(str)` for display-related calculations | Byte length ≠ character length; breaks CJK, emoji, accented chars | §1.5, §4.5 |

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

## 11. Pre-Merge Checklist

Before completing any change, mechanically verify:

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
- [ ] Data integrity failures stop execution and surface explicit errors to the user.
- [ ] No silent error swallowing — every error is wrapped with context or explicitly annotated.
- [ ] No file exceeds 500 LoC.
- [ ] No `len()` used for display-related string length; `utf8.RuneCountInString` used instead.
- [ ] No page holds rendering state that belongs to a child component.