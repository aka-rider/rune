# QA Instructions — Editor Spec Conformance Testing

> Referenced from `CLAUDE.md` §8. This document is the testing authority for `pkg/editor/`, `pkg/command/`, and `pkg/ui/components/editor/`.

---

## Governing Principle

Tests validate **specification conformance**, not code coverage. A test is valuable when its failure means "the editor violates its spec" — not "the implementation changed." Every test must trace to a sentence in `editor-spec.md`.

---

## Test Layers

| Layer | What it proves | Mechanism | Naming |
|-------|---------------|-----------|--------|
| 1. Invariants | Properties hold for ALL inputs | Fuzz (`testing.F`) + property loops | `Fuzz*`, `TestInvariant_*` |
| 2. Spec Scenarios | Each command behaves as specified | Table-driven subtests | `TestSpec_*` |
| 3. Integration | Full keystroke→display pipeline works | Editor model Update/View cycle | `TestIntegration_*` |
| 4. Golden | Rendered output doesn't regress | `testdata/*.golden` file comparison | `TestGolden_*` |

---

## Architecture Rules

### No Circular Imports

Test helpers live in `internal/editortest/`. This package works ONLY with primitive data (strings, int offsets). It MUST NOT import `pkg/editor/buffer`, `pkg/editor/cursor`, or any editor package.

Each package's `*_test.go` bridges from `editortest` types to its own types.

### State Notation (ParseState / FormatState)

A single string encodes content + cursors + selections:

| Marker | Meaning |
|--------|---------|
| `\|` | Cursor (no selection) |
| `[text]` | Forward selection (anchor=`[`, position=`]`) |
| `]text[` | Backward selection (position=`]`, anchor=`[`) |
| `\\|`, `\\[`, `\\]` | Literal characters in content |

Examples:
- `"hel|lo"` → content `"hello"`, cursor at offset 3
- `"h[ell]o"` → content `"hello"`, selection [1,4)
- `"a|b|c"` → content `"abc"`, two cursors at 1 and 2

### Deterministic Time

History coalescing uses a `func() time.Time` injection. Tests use `editortest.Clock` (value type, `Advance` returns new Clock). **Never `time.Sleep`.**

### No External Test Dependencies

Standard library `testing` only. No testify, no gomock. Use `t.Helper()` + `t.Fatalf` with diff output.

---

## Layer 1: What Invariants to Test

### Buffer (`pkg/editor/buffer`)
1. `Len() == len(String())` — always
2. Old Buffer values unchanged after edit (snapshot immutability)
3. `ApplyEdits` in batch == same edits applied individually in reverse-offset order
4. `FromBytes` rejects invalid UTF-8 with error (never silent replacement)
5. Line index consistency: `LineStart(n) + len(Line(n))` aligns with content

### Cursor (`pkg/editor/cursor`)
1. All positions in `[0, bufLen]` after any operation
2. After `Merge()`: sorted ascending, no overlapping selections
3. `AdjustAfterEdit` preserves relative ordering of non-overlapping cursors

### History (`pkg/editor/history`)
1. N pushes + N undos = original content AND original cursor state
2. Undo then Redo = identity
3. New edit after undo clears redo stack completely

### Coordinates (`pkg/editor/coords` + `pkg/editor/display`)
1. `BufferToSyntax` → `SyntaxToBuffer` = identity for all valid offsets
2. `SyntaxToWrap` → `WrapToSyntax` = identity for all valid syntax points
3. Monotonicity: buf offset increases → display position never decreases (within same line)

---

## Layer 2: Spec→Test Recipe

For EACH command row in `editor-spec.md` action tables:

1. **Happy path** — operation works mid-content
2. **Start boundary** — cursor at offset 0, or line start
3. **End boundary** — cursor at buffer end, or line end
4. **Selection interaction** — what changes when selection is active?
5. **Multi-cursor** — does it fan out? Does reverse-offset apply?
6. **Empty buffer** — does it no-op or error gracefully?

Minimum: 4 entries per command. Target: ~300 total test entries across all commands.

### Critical Commands Requiring Extra Scrutiny

| Command | Why extra tests needed |
|---------|----------------------|
| `edit.delete-left` | Line-join semantics, selection delete, multi-cursor offset shift |
| `edit.move-line-up/down` | Block selection expansion, boundary at first/last line |
| `edit.indent/outdent` | Multi-line selection behavior differs from single-cursor |
| `clipboard.paste` | N-lines-to-N-cursors distribution logic |
| `multicursor.add-above/below` | DesiredCol clamping to short lines |
| All `select.*` variants | Anchor stability, direction reversal, collapse behavior |

---

## Layer 3: Integration Test Patterns

Integration tests use the full `editor.Model` (Bubble Tea component). They:
1. Construct via `editor.New(keys, styles, registry)`
2. Call `SetSize` and `SetFocused(true)`
3. Feed `tea.KeyPressMsg` events
4. Assert on `View()` output or extracted content

```go
// Pattern:
m := newTestEditor("content")
m = feedKeys(m, "typed text")
m = pressKey(m, ctrlZ)
assertContent(t, m, "content")
```

---

## Layer 4: Golden File Protocol

- Golden files live in each package's `testdata/golden/` directory
- Run with `-update` flag to regenerate: `go test ./... -run Golden -update`
- Golden tests are SKIPPED until the display pipeline is stable (Phase 6+)
- Never golden-test raw ANSI escapes — strip them or test semantic content only

---

## File Locations

```
internal/editortest/
  notation.go              # ParseState, FormatState
  clock.go                 # Deterministic Clock value type
  golden.go                # GoldenFile helper with -update flag
  diff.go                  # Unified diff for assertion output
pkg/editor/buffer/
  buffer_test.go           # Layers 1+2
pkg/editor/cursor/
  cursor_test.go           # Layers 1+2
pkg/editor/history/
  history_test.go          # Layers 1+2
pkg/editor/coords/
  coords_test.go           # Layer 1
pkg/editor/display/
  display_test.go          # Layers 2+3
  testdata/golden/         # Layer 4
pkg/editor/keybind/
  resolver_test.go         # Layer 2
pkg/command/
  registry_test.go         # Layer 2
pkg/ui/components/editor/
  editor_test.go           # Layers 3+4
  testdata/golden/         # Layer 4
```

---

## Verification

```bash
# Full test suite
go test ./pkg/editor/... ./pkg/command/... ./internal/editortest/...

# Fuzz (run 30s per target)
go test ./pkg/editor/buffer/ -fuzz Fuzz -fuzztime 30s

# Golden update
go test ./pkg/ui/components/editor/ -run Golden -update

# LoC check (CLAUDE.md §1.4: 500 max)
find . -name '*_test.go' | xargs wc -l | awk '$1 > 500 {print "VIOLATION:", $0}'
```

---

## Anti-Patterns to Reject in Review

| Anti-pattern | Why wrong |
|-------------|-----------|
| Test asserts internal struct field values | Tests spec behavior, not implementation |
| Test imports buffer package from editortest | Circular import |
| `time.Sleep` anywhere in test | Use deterministic clock |
| Golden test checks ANSI escape bytes | Brittle; test semantic content |
| Single test case per command | Misses boundaries where bugs live |
| Test mirrors implementation logic | "Coverage" without conformance value |
| `reflect.DeepEqual` on Model structs | Test observable behavior (content, cursors, view) |
