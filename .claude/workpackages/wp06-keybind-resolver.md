# WP6 — Keybinding Resolver

## Scope

`pkg/editor/keybind/`

## Dependencies

None (can run in parallel with WP0, WP1, WP5)

## Deliverables

### `pkg/editor/keybind/resolver.go`

Full API from spec §J:

```go
package keybind

type Chord struct {
    Ctrl  bool
    Shift bool
    Alt   bool
    Cmd   bool
    Key   string  // "left", "right", "a", "backspace", "k", etc.
}

func ChordFromKeyMsg(msg tea.KeyPressMsg) Chord

type Binding struct {
    Chords  []Chord  // length 1 = single combo; length 2+ = multi-step chord
    Command string
    When    string   // context predicate (empty = always active)
}

type ResultKind int
const (
    ResultNoMatch ResultKind = iota
    ResultMoreChordsNeeded
    ResultFound
)

type ResolutionResult struct {
    Kind    ResultKind
    Command string  // set when Kind == ResultFound
}

type ResolverContext struct {
    EditorFocused  bool
    HasSelection   bool
    HasMultiCursor bool
    InCodeFence    bool
    ReadOnly       bool
}

type Resolver struct { /* bindings, index map, pending chords */ }

func NewResolver(bindings []Binding) (Resolver, error)
func (r Resolver) Resolve(chord Chord, ctx ResolverContext) (Resolver, ResolutionResult)
func (r Resolver) ResolveTimeout() (Resolver, ResolutionResult)
func (r Resolver) Reset() Resolver
func (r Resolver) InChordMode() bool
func (r Resolver) PendingDisplay() string  // e.g. "Ctrl+K ..."
```

### Resolution Algorithm (from spec)

1. Append incoming Chord to `pending`
2. Find all bindings where `bindings[i].Chords[:len(pending)]` equals `pending` (prefix match)
3. Filter by `When` predicates against context
4. Zero candidates → `ResultNoMatch`, reset pending
5. Exact full match AND no longer candidate → `ResultFound`
6. All remaining have longer Chords → `ResultMoreChordsNeeded`
7. Mixed (some exact, some longer) → `ResultMoreChordsNeeded`; on timeout (1500ms), fire shortest full match

### `When` Predicate Evaluation

The `When` field is a simple boolean expression string supporting:
- Identifiers: `editorFocused`, `hasSelection`, `hasMultiCursor`, `inCodeFence`, `readOnly`
- Operators: `&&` (AND), `||` (OR), `!` (NOT)
- Parentheses for grouping
- Empty string = always active

Implement this exact expression grammar; do not use regex-only parsing:

```text
Expression := Or
Or         := And { "||" And }
And        := Unary { "&&" Unary }
Unary      := "!" Unary | Primary
Primary    := Identifier | "(" Expression ")"
Identifier := "editorFocused" | "hasSelection" | "hasMultiCursor" | "inCodeFence" | "readOnly"
```

Whitespace is allowed between tokens. Empty string = always active. Unknown identifiers, malformed expressions, or trailing tokens must return an error during resolver construction. Do not silently treat typos as false.

### `pkg/editor/keybind/resolver_test.go`

- Single-chord exact modifier match (`Alt+→` → "cursor.word-right")
- Different modifier sets don't collide (`Alt+→` vs `Alt+Ctrl+→`)
- Multi-step chord: first press → pending, second press → found
- Context predicate filtering (unfocused → no-match)
- Malformed `When` predicate and unknown identifier fail construction with a clear error
- No-match resets pending state
- Timeout resolution: fires shortest full match
- Collision pair validation from spec:
  - `Alt+↑` (move) vs `Alt+Shift+↑` (clone) — distinct
  - Selection + Multi-Cursor composition

## Constraints

- Resolver does NOT define physical bindings — receives command bindings derived from keymap
- Exact modifier equality matching (no prefix for single chords)
- Value semantics (Resolve returns new Resolver)
- Resolver validates duplicate command chord sequences within its binding list and returns construction errors for invalid bindings/predicates. Physical key-string collisions across `pkg/ui/keymap/keymap.go` are verified by keymap tests in WP08 before editing keys land.
- Under 500 LoC per file

## QA Gates

These gates protect WP8 (editor dispatches all input through resolver) and WP13 (workspace key routing depends on resolution correctness).

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | No duplicate `Chord` sequence maps to two commands within the resolver input for the same context | Ambiguous dispatch → same normalized chord can execute different commands |
| 2 | Modifier exactness: `Alt+Right` resolves to `cursor.word-right`, `Ctrl+Alt+Right` resolves to no-match | Extra modifier accidentally triggers wrong command → user loses text |
| 3 | Multi-step chord: first keypress returns `ResultMoreChordsNeeded`, full sequence returns `ResultFound` with correct command | Chord sequence fires too early (on first key) → wrong command executes |
| 4 | After timeout with pending chord that has a complete match: returns the shortest full match | User pauses during chord → timeout fires nothing (action lost) or fires wrong command |
| 5 | `When` predicate evaluation: command with `when: "editorFocused"` does NOT match when editor is unfocused | Unfocused editor executes commands intended for focused state → invisible edits to background buffer |

**Testing approach:** Table-driven with concrete bindings, modifier sets, and context predicates. Gate 1 via static analysis of binding list at construction.

## Verification

```bash
go test ./pkg/editor/keybind/ -v
```
