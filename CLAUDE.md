# Claude Instructions

These instructions define the architectural principles, patterns, and rules for building `rune`, a Bubble Tea based TUI markdown editor and note-taking app.

## Core Principles

### Value Semantics & Concurrency
- **Value semantics by default:** Use Go values over pointers to avoid nil-checks, memory aliasing, and concurrency races. Only use pointers when absolutely necessary (e.g., when a struct contains a `sync.Mutex` or owns a unique resource).
- **Share memory by communicating:** Never communicate by sharing memory. Data crossing goroutine boundaries must be passed by copy or be strictly immutable.
- **Meaningful Zero Values:** Take advantage of zero values to make them meaningful. Instead of returning pointers to signal absence (e.g., `func Get() *Type` with a nil check), use structs with validity flags (e.g., `type Type struct { ... Valid bool }` so `func Get() Type` allows checking `if v := Get(); v.Valid`).

### Error Handling & Data Integrity
- **Fail Fast on Data Risk:** Data integrity is paramount. If there is *any* suspicion that a user's data may be corrupted or lost (e.g., disk full when writing notes), the app must STOP operations to prevent further damage. Be very vocal and explicit to the user.
- **Graceful Degradation:** Only fallback or degrade gracefully when the app is 100% sure the state is recoverable (e.g., a failed HTTP request to fetch a preview). 
- **No Swallowed Errors:** **NEVER** swallow or ignore errors. Bare ignored errors (e.g., `_, _ = operation()`) are strictly prohibited unless explicitly documented with a `// fire-and-forget: <reason>` comment.
- **No Silent Fallbacks:** Do not default or fallback silently when encountering invalid user-supplied data or paths.
- **Contextual Wrapping:** Always wrap errors with operation and resource context (e.g., file path, operation name) before returning them so the user sees *why* it failed.

### Project Organization & Clean Code
- **No Catch-All Packages:** Never create `types`, `utils`, `helpers`, or `misc` packages. 
- **One Entity Per File:** Each file should own one primary type and its immediate helpers.
- **File Length Code Smell:** A 500-line limit is a strong code smell. If a UI component exceeds 500 LoC, it must be broken down into smaller sub-components. The ELM architecture encourages working in small, immutable models.

## TUI Architecture (Bubble Tea)

This application strictly adheres to The Elm Architecture via Bubble Tea.

### State Management & Components
- **No God-Structs:** The global, top-level TUI `Model` must not keep any global state. It should only act as a router, delegating messages to child components. Actual TUI state must be spread across specialized component models.
- **No Aliasing:** Models own their child models by value. Do not store pointers to other models.
- **Dependency Injection:** Pass styles, keymaps, and layout dimensions via constructors. Do not store `context.Context` (pass it as a function parameter if absolutely required for a specific non-model function) or `*Logger` fields on models.

### The Elm Cycle (`Init`, `Update`, `View`)
- **Value Receivers:** `Init`, `Update`, and `View` methods must use value receivers (e.g., `func (m Model) Update(...) (Model, tea.Cmd)`). Avoid pointer receivers (`*Model`) and interface boxing. `Update` returns a modified copy of the model.
- **Pure `View` Functions:** `View()` is strictly read-only. It must not contain variable assignments, I/O operations, or emit `tea.Cmd`. It must not mutate focus or dimensions (no `SetWidth` or `SetHeight` inside `View`).
- **Non-Blocking `Update`:** Do not perform I/O, sleeps, or long parsing directly in `Init()` or `Update()`. Offload blocking work via `tea.Cmd` and return a typed completion message.

### Layout & Rendering
- **Calculate Dimensions in Update:** Handle `tea.WindowSizeMsg` by calculating all layout dimensions in a specific update method called from `Update()`. 
- **No Rendering for Measurement:** Do not render just to measure inside `Update()`.
- **Viewport Scroll:** Treat `viewport.SetContent()` as scroll-destructive. Capture `AtBottom()` or the previous offset before setting content and restore only when that behavior is deliberate.
- **Mouse Event Routing:** Mouse events must be routed through tracked bounds (`image.Rect`) before forwarding to panes so background viewports cannot consume foreground interactions.

### Testing Enforcement
When changing TUI behavior, add focused tests for the invariant class:
- **Render Purity:** Call `View()` repeatedly and assert viewport sizes, offsets, focus state, and content hashes do not change.
- **Layout & Resizing:** Test small terminals, resize transitions, and ensure no overlap between tracked bounds.
- **Scroll Stability:** Test user-scrolled viewports across ticks or content updates.
- **Async IO:** Test command messages for success and failure, including missing files and parse errors. Do not use `time.Sleep` for synchronization.

### Commands & Messages
- **Side Effects via Commands:** All side effects must be wrapped in `tea.Cmd` and return results as strongly-typed `tea.Msg` structs. Messages should be defined in the producer's package.
- **Forwarding and Batching:** Parent components must forward relevant messages (like `tea.WindowSizeMsg`) to all children. When returning from `Update`, combine all child commands using `tea.Batch`. Never silently drop a `Cmd`.
- **Closure Safety:** When defining `tea.Cmd` closures, capture local variables by value. Do not capture model fields directly, as they may lead to race conditions.

### Directory Structure

The UI components should strictly follow this directory structure:
- `pkg/ui/` - Top level router/app model
- `pkg/ui/components/` - Reusable, isolated UI widgets
- `pkg/ui/pages/` - Full screen views composed of components
- `pkg/ui/styles/` - Shared styling definitions
- `pkg/ui/keymap/` - Shared keybindings (injected into components, never built inline)

*Components must not import pages or domain packages. Pages can import components.*

## Pre-Merge Verification
Before completing any changes, verify:
- [ ] No pointer receivers in TUI methods (`Init`, `Update`, `View`).
- [ ] No `context.Context` or `*Logger` fields on models.
- [ ] All `tea.Cmd` from child components are batched and returned.
- [ ] `tea.WindowSizeMsg` is forwarded to all child components that need it.
- [ ] `View()` does not mutate state or return commands.
- [ ] Data integrity failure cases explicitly stop execution and warn the user.
- [ ] No silent error swallowing or generic `if err != nil { return err }` without context.