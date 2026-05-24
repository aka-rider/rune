# QA Strategy for Rune Editor

## Philosophy

Testing is not QA. Testing is one tool of QA. This document defines a **quality assurance strategy** — it answers:

1. **Where will quality degrade?** (Risk analysis)
2. **What user harm does each failure mode cause?** (Impact ranking)
3. **What testing approach catches each class of defect?** (Methodology)
4. **How do we know when quality is sufficient?** (Acceptance criteria)

Every test in this project must answer: "If this test fails, what user harm was prevented?" If the answer is "none — it just proves an internal data structure is consistent," the test does not belong here.

---

## Part 1: Risk Analysis

Five risk zones, ranked by severity of user harm.

### Risk Zone 1: Data Integrity (Critical)

**What goes wrong:** User's text is silently corrupted, lost, or mangled. Undo doesn't restore previous state. File save writes different bytes than the buffer contains.

**User harm:** Work is destroyed. Trust is broken permanently.

**Where bugs live:**
- Multi-cursor edit algorithm applying offsets incorrectly (earlier edit shifts later cursor positions)
- Undo stack recording wrong inverse operations
- UTF-8 validity violation producing mojibake on save/reload
- Buffer line index desynchronizing from content after edits
- File save writing stale buffer content (race between edit and flush)

**Testing approach:** Property-based testing (fuzz) with universal invariants. The "trust test" — load → N operations → undo ALL → byte-identical to original.

### Risk Zone 2: Coordinate Pipeline (High)

**What goes wrong:** The screen lies to the user. Cursor appears at wrong position. Selection highlight doesn't match what will be deleted. Click lands on wrong character.

**User harm:** User operates on wrong text. Deletes the wrong thing. Selects the wrong range. Loses trust in visual feedback.

**Where bugs live:**
- Buffer→Syntax offset delta miscalculation when delimiters are hidden/shown
- Syntax→Wrap mapping at soft-wrap boundaries (off-by-one on continuation rows)
- Tab expansion calculating wrong cell width (depends on column position, not absolute)
- CJK double-width characters causing cursor drift
- Hidden delimiter boundaries: cursor movement INTO a hidden region should skip, not land inside
- Viewport scroll offset applied incorrectly in Display→Buffer reverse mapping (mouse clicks)

**Testing approach:** Round-trip property tests (forward→inverse = identity). Monotonicity invariants. Specific scenarios for tab/CJK/delimiter boundaries.

### Risk Zone 3: Multi-Cursor Editing (High)

**What goes wrong:** Edits applied to wrong positions. Cursor merge loses cursors unexpectedly. Adjacent cursor operations corrupt each other's text regions.

**User harm:** User adds cursors, types, and gets garbled text in some positions. Or worse: some cursors' edits silently eat adjacent cursors' text.

**Where bugs live:**
- Reverse-offset sort: edits MUST apply from highest offset to lowest, or each edit shifts subsequent positions
- Cursor merge after edit: two cursors land on same position but only one survives
- Undo after merge: restoring N cursors from N-1 state
- add-below/above: DesiredCol clamping to short lines miscalculates buffer position
- Line-range unification for block operations (move-line with cursors on adjacent lines)

**Testing approach:** Concrete byte-offset scenarios for the reverse-offset algorithm. Merge postcondition assertions. Round-trip with undo.

### Risk Zone 4: Undo/Redo (High)

**What goes wrong:** Undo doesn't restore previous state. Redo stack isn't cleared after new edit. Coalescing groups wrong operations together (user can't undo one word, only 5 seconds of typing).

**User harm:** User cannot recover from mistakes. The safety net has holes.

**Where bugs live:**
- Inverse operation calculation: `InverseEdits()` producing wrong offsets or wrong deleted text
- Coalescing at timing boundary: 299ms vs 301ms determining group membership
- Coalescing across multi-cursor transitions (1 cursor → 2 cursors mid-group)
- Redo stack not cleared: user undoes, makes new edit, presses redo and gets stale state
- Cursor restoration: undo restores content but cursors are at wrong positions

**Testing approach:** The "trust test" (universal: any sequence of ops + full undo = original). Coalescing boundary tests with deterministic clock. Explicit redo-invalidation proof.

### Risk Zone 5: Selection & Clipboard (Medium)

**What goes wrong:** Copy captures wrong text. Paste goes to wrong location. Selection highlight shows one range but operation affects different bytes.

**User harm:** User copies "important text" but pastes garbage. Clipboard workflow breaks.

**Where bugs live:**
- Copy with no selection: should copy entire line including `\n`, but might miss the `\n`
- Paste distribution: N lines into N cursors should distribute 1-per-cursor, but M cursors ≠ N lines should paste full text at each
- Selection direction: forward `[text]` vs backward `]text[` affects which end the cursor collapses to
- Selection spanning hidden delimiters: visual range ≠ byte range

**Testing approach:** Table-driven scenarios with explicit clipboard state tracking. Distribution edge cases.

---

## Part 2: Universal Properties

Eight user-facing behavioral invariants. Each states what user harm it prevents.

### P1: Cursor Bounds

> No operation produces a cursor with Position or Anchor outside `[0, buffer.Len()]`.

**Harm prevented:** Out-of-bounds cursor causes panic on next operation, or silent corruption if used as edit offset.

**Test approach:** Fuzz — random valid content + random operations → assert all cursor fields in bounds after each operation.

### P2: UTF-8 Preservation

> No sequence of editing operations produces invalid UTF-8 from valid UTF-8 input.

**Harm prevented:** Invalid UTF-8 in buffer corrupts file on save. Other components (display pipeline, clipboard) may panic or produce mojibake.

**Test approach:** Fuzz — random valid UTF-8 content + random Insert/Delete/Replace with valid UTF-8 text → `utf8.ValidString(result)` always true.

### P3: Undo Completeness

> For ANY sequence of N operation groups, N undos restore the EXACT original state: byte-identical content AND all cursor fields (Position, Anchor, DesiredCol) match pre-operation values.

**Harm prevented:** Undo lies — user thinks they've recovered but content or cursor positions are subtly wrong.

**Test approach:** Property loop — generate random operation sequences, undo all, compare byte-for-byte. This is the "trust test."

### P4: Redo Faithfulness

> For ANY undo, a subsequent redo restores the exact pre-undo state. After undo+redo, the model is byte-identical to before the undo.

**Harm prevented:** Redo produces different state than what was undone, confusing the user.

**Test approach:** Property loop — random ops, undo K of them, redo K, compare to state after original ops.

### P5: Coordinate Round-Trip

> For cursor-legal positions (not inside hidden delimiters), `Display→Buffer→Display` produces the same coordinates. For all valid buffer offsets, `Buffer→Syntax→Buffer` is identity.

**Harm prevented:** Cursor appears at wrong screen position. Mouse clicks map to wrong buffer offset. Visual selection doesn't match byte range.

**Test approach:** For each coordinate pair (Buf↔Syntax, Syntax↔Wrap, Wrap↔Display): fuzz over valid positions, assert round-trip identity. Test with and without SyntaxMap folding.

### P6: Multi-Cursor Edit Correctness

> Multi-cursor edits sorted descending by offset produce the correct combined result. The result is independent of the order cursors were added — only their positions matter.

**Harm prevented:** Adding cursors in different order produces different text. Edits at higher offsets corrupt text at lower offsets.

**Test approach:** Property test — generate N cursors at random non-overlapping positions, apply same edit, verify each cursor's surrounding text is correct.

### P7: Buffer Length Consistency

> `buffer.Len()` always equals `len(buffer.String())` after any operation.

**Harm prevented:** Line index, cursor bounds checks, and offset calculations use `Len()` — if it disagrees with actual content length, every bounds check is wrong.

**Test approach:** Assert after every buffer operation in property tests. Trivial but load-bearing.

### P8: Selection Bounds

> After any operation, for every cursor: `min(Position, Anchor)` ≥ 0 and `max(Position, Anchor)` ≤ `buffer.Len()`.

**Harm prevented:** Selection operations (copy, cut, delete) use anchor/position as byte range. Out-of-bounds selection causes panic or corrupts adjacent text.

**Test approach:** Included in P1 fuzz — check both fields of every cursor after every operation.

---

## Part 3: Compound Scenario Matrix

Single-operation tests ("insert X → X is there") pass by construction in any non-broken implementation. Bugs live at **feature intersections**. This section defines which intersections to test.

### Matrix Axes

| Axis | Values |
|------|--------|
| **Operation** | insert, delete-left, delete-right, delete-word-left, delete-word-right, newline, paste, move-line-up, move-line-down, clone-line, indent, outdent |
| **Context** | single-cursor, multi-cursor-spaced, multi-cursor-adjacent, selection-forward, selection-backward, multi-line-selection |
| **Boundary** | doc-start, doc-end, line-start, line-end, word-boundary, empty-buffer, single-char-buffer |
| **Complication** | with-tabs, with-unicode-multibyte, at-soft-wrap-break, at-hidden-delimiter, after-undo |

### High-Risk Cells (Mandatory Coverage)

These are the intersections where bugs will actually live. Each cell traces to a specific ambiguity or compounding behavior.

#### delete-left × multi-cursor-adjacent × line-start

**Why risky:** Two cursors on adjacent lines, both at line start. First cursor joins line above (reducing line count). Second cursor's line reference is now wrong.

```
Initial:  "aaa\n|bbb\n|ccc"  (cursors at line-start of lines 2 and 3)
Expected: "aaa|bbb\n|ccc"    — OR — "aaa|bbb|ccc" (both join?)
```

The spec says each cursor independently executes delete-left. With reverse-offset application, cursor 2 (higher offset) executes first: deletes \n between lines 2 and 3 → "aaa\n|bbcccc". Then cursor 1 (lower offset) deletes \n between lines 1 and 2 → "aaa|bbbccc". Both newlines deleted. This test verifies the algorithm handles line-joining under reverse-offset correctly.

#### paste × multi-cursor × N-lines-to-M-cursors (N≠M)

**Why risky:** The distribution rule (N lines → N cursors: one per cursor) has an edge case when line count doesn't match cursor count.

```
# 3 lines, 2 cursors → full text at each (no distribution)
Initial:   "aa|bb\ncc|dd"
Clipboard: "X\nY\nZ"
Expected:  "aaX\nY\nZ|bb\nccX\nY\nZ|dd"

# 2 lines, 3 cursors → full text at each (spec: distribute ONLY when N==M)
Initial:   "a|b|c|d"
Clipboard: "X\nY"
Expected:  "aX\nY|bX\nY|cX\nY|d"
```

#### move-line-up × multi-cursor-on-adjacent-lines × first-line

**Why risky:** Cursors on lines 0 and 1. Line 0 can't move up (no-op). Line 1 should move up (swap with line 0). But they're in the same block after unification — does the entire block no-op?

```
Initial:  "a|aa\nb|bb\nccc"  (cursor on line 0 and line 1)
Expected: "a|aa\nb|bb\nccc"  (entire block at top → no-op)
```

#### newline × multi-cursor × with-indented-lines

**Why risky:** Each cursor should copy whitespace from ITS OWN current line, not from some shared state. If auto-indent reads from the buffer after the first cursor's newline was applied, subsequent cursors get wrong indent.

```
Initial:  "  hel|lo\n    wor|ld"
Expected: "  hel\n  |lo\n    wor\n    |ld"  (each gets indent of its own line)
```

#### insert × at-soft-wrap-break × causes-rewrap

**Why risky:** Inserting a character at the exact soft-wrap boundary changes which characters are on which display row. Cursor must stay with its text, not jump to wrong display row.

The display row changes but the buffer offset remains correct — the display pipeline must recalculate after the edit.

#### delete-line × multi-cursor × all-cursors-same-line

**Why risky:** After unification to a single line block, one line is deleted. All N cursors merge to 1.

```
Initial:  "aaa\nb|b|b\nccc"  (2 cursors on line 1)
Expected: "aaa\n|ccc"         (line deleted, cursors merge to 1)
```

#### clone-line-down × buffer-without-trailing-newline

**Why risky:** Cloning the last line requires inserting \n + content. Buffer previously didn't end with \n.

```
Initial:  "hel|lo"    (no trailing \n)
Expected: "hel|lo\nhello"  (\n inserted between original and clone)
```

#### select-all × multi-cursor

**Why risky:** Each cursor independently selects all? That produces N identical full-document selections → merge to 1.

```
Initial:  "he|ll|o"   (2 cursors)
Expected: "[hello]"    (merged to single cursor selecting entire document)
```

#### indent × multi-line-selection × selection-starts-mid-line

**Why risky:** Does a partially-selected first line get indented? Spec says "multi-line selection: indent all."

```
Initial:  "hel[lo\nworld]"
Expected: "\thel[lo\n\tworld]"  (both lines indented — selection spans them)
```

#### word-left × with-unicode-multibyte

**Why risky:** Word boundary is ASCII `[a-zA-Z0-9_]` transitions. `café` has non-ASCII `é`.

```
Initial:  "café| world"
Expected: "caf|é world"   (é is non-word per ASCII rule, boundary between f and é)
```

### Additional High-Risk Scenarios

```
# DesiredCol preservation across lines of different length
{"desired-col/long-short-long", "hello worl|d\nhi\nhello world", "down,down", "hello worl|d"}
# Cursor at col 10 → line 2 (len=2, clamps) → line 3 (restores col 10)

# Selection collapse direction
{"collapse-fwd-left",  "he[llo] world", "cursor.character-left",  "he|llo world"}
{"collapse-fwd-right", "he[llo] world", "cursor.character-right", "hello| world"}
{"collapse-bwd-left",  "he]llo[ world", "cursor.character-left",  "he|llo world"}
{"collapse-bwd-right", "he]llo[ world", "cursor.character-right", "hello| world"}

# move-line-up preserves cursor column
{"move-up-col", "aaa\n  hel|lo\nccc", "edit.move-line-up", "  hel|lo\naaa\nccc"}

# Undo after cursor merge
# Adjacent cursors at 1,2 in "ab" → delete-left → merge → undo restores BOTH
{"undo-after-merge", "a|b|c", "delete-left then undo", "a|b|c"}
```

---

## Part 4: Coordinate Pipeline Verification

The 4-stage pipeline (Buffer → Syntax → Wrap → Display) is the source of ALL visual correctness.

### Round-Trip Invariants

| Conversion | Invariant | Condition |
|------------|-----------|-----------|
| Buffer→Syntax→Buffer | Identity | All valid offsets |
| Syntax→Buffer→Syntax | Identity | Offsets NOT inside hidden delimiters |
| Syntax→Wrap→Syntax | Identity | All cursor-legal wrap positions |
| Wrap→Display→Wrap | Identity (add/subtract TopRow) | All visible positions |
| Buffer→Display→Buffer | Identity | Cursor-legal, visible, not inside hidden tokens |

### Monotonicity

For any two buffer offsets `a < b` on the same model line:
- `SyntaxCol(a) ≤ SyntaxCol(b)` (equal only if `b` is inside a hidden token that `a` precedes)
- `WrapCol(a) ≤ WrapCol(b)` when on the same wrap row
- `DisplayCol(a) ≤ DisplayCol(b)` when on the same display row

### Tab Expansion Scenarios

Tabs have position-dependent width (next tab stop at multiples of 4):

| Line content | Tab at col | Visual width | Cursor after tab |
|---|---|---|---|
| `\thello` | col 0 | 4 cells | col 4 |
| `x\thello` | col 1 | 3 cells | col 4 |
| `xx\thello` | col 2 | 2 cells | col 4 |
| `xxx\thello` | col 3 | 1 cell | col 4 |
| `xxxx\thello` | col 4 | 4 cells | col 8 |

### CJK Double-Width

| Content | Buffer offset of char | Display column |
|---|---|---|
| `A中B` | A=0, 中=1(start of 3-byte seq), B=4 | A=col0, 中=col1-2, B=col3 |

Cursor between CJK characters lands at cell boundary, never in the middle of a wide char.

### Hidden Delimiter Boundaries

With SyntaxMap hiding `**` in `**bold**`:

- Cursor must NEVER rest on a hidden byte offset
- `character-right` from just before hidden `**`: skip to first visible char after delimiter
- `character-left` from just after hidden `**`: skip to last visible char before delimiter
- Buffer offset inside `**` clamps to nearest visible position in display conversion

### Soft-Wrap Boundary

Line `"hello world"` with wrap width 6:
- Row 0: `"hello "` (6 chars)
- Row 1: `"world"` (5 chars)

Cursor at buffer offset 6 (`w`) → wrap row 1, col 0. Moving left → wrap row 0, col 5 (the space). The display row changes but buffer offset arithmetic is straightforward.

---

## Part 5: Undo/Redo Deep Verification

### The Trust Test

The single most important test in this project:

```
For ANY sequence of N editing operations:
  1. Record initial state (content + all cursor fields)
  2. Apply all N operations
  3. Undo exactly UndoStackLen() groups
  4. Assert final state is BYTE-IDENTICAL to initial state (content + cursors)
```

Run with:
- N = 1..50 (varying operation counts)
- Random operation types (insert, delete, newline, indent, move-line, clone-line)
- Random cursor positions (valid)
- Random multi-cursor configurations (1..5 cursors)
- Deterministic clock controlling coalescing
- 1000 random sequences per test run

### Coalescing Boundary Conditions

Using deterministic clock:

| Sequence | Time gaps | Expected groups |
|----------|-----------|----------------|
| `a, b, c` | 100ms, 100ms | 1 (all coalesce) |
| `a, b, c` | 100ms, 400ms | 2 (`ab` + `c`) |
| `a, <space>` | 50ms | 2 (whitespace forces break) |
| `a, <backspace>` | 50ms | 2 (delete forces break) |
| `a, <enter>` | 50ms | 2 (newline forces break) |
| `a, b` then paste `XY` | 50ms, 50ms | 2 (paste forces break) |
| move-line-up | — | Always own group |
| clone-line-down | — | Always own group |
| multi-cursor insert at 3 positions | — | 1 (single command = single group) |

### Undo Across Cursor-Count Changes

```
Initial:      "a|b|c" (2 cursors at offsets 1, 3)
Operation:    delete-left at each (reverse-offset)
After edit:   cursor at 3 deletes 'b' → "a|c", cursor at 1 deletes 'a' → "|c"
              But with proper reverse-offset: higher first → del at 2 → "a|c" → del at 0 → "|c"
              If cursors collide → merge
Undo:         Must restore BOTH cursors at original positions, content "abc"
```

The undo stack stores `CursorsBefore`. Undo restores that exactly, regardless of current cursor count.

### Redo Invalidation Proof

```
For ANY operation sequence:
  apply(ops)
  undo(K)                        // K ≤ undoStackLen
  assert(redoStack.Len() == K)
  apply(anyNewOp)                // any editing operation
  assert(redoStack.Len() == 0)   // ALWAYS cleared
  redo()                         // must be no-op
  assert(state unchanged)
```

---

## Part 6: Integration Workflows

Real user sessions as end-to-end test sequences.

### Workflow 1: Write a Paragraph

```
1. Load empty buffer
2. Type "Hello, world. " (14 chars + space → space forces new undo group)
3. Type "This is a test." (15 chars, all within 300ms)
4. Undo → removes "This is a test." (one coalesced group)
5. Assert: content == "Hello, world. ", cursor at 14
6. Type "Goodbye."
7. Redo → no-op (redo cleared by step 6)
8. Assert: content == "Hello, world. Goodbye."
```

### Workflow 2: Multi-Cursor Rename

```
1. Load "foo bar foo baz foo"
2. Place cursors at offsets 0, 8, 16 (start of each "foo")
3. Create forward selections: [foo] at each
4. Type "qux" → replaces each selection
5. Assert: content == "qux bar qux baz qux"
6. Undo (single group)
7. Assert: content == "foo bar foo baz foo", all 3 selections [foo] restored
```

### Workflow 3: Reorganize Document

```
1. Load "line1\nline2\nline3\nline4"
2. Cursor on line 3 mid-line
3. move-line-up → "line1\nline3\nline2\nline4"
4. move-line-up → "line3\nline1\nline2\nline4"
5. move-line-up (at top → no-op)
6. Undo 2 times (each move-line is own group)
7. Assert: "line1\nline2\nline3\nline4", cursor back on line 3
```

### Workflow 4: Clipboard

```
1. Load "aaa\nbbb\nccc"
2. Cursor on line 2 ("bbb"), no selection
3. Copy → clipboard == "bbb\n" (entire line including \n)
4. Move cursor to end of document
5. Paste → content ends with "bbb\n" pasted at cursor
6. Undo → content restored to "aaa\nbbb\nccc"
```

### Workflow 5: Recover from Mistake

```
1. Load "important data\ndo not lose this\nkeep this too"
2. Select all (Cmd+A)
3. Type "oops" → replaces entire document
4. Undo → content fully restored, selection restored
5. Assert: byte-identical to original
```

### Workflow 6: Resize Mid-Edit

```
1. Load long line, set viewport width to 20 (soft-wrap active)
2. Create selection spanning wrap boundary (offset 10-30)
3. Resize to width 40 (re-wrap)
4. Assert: selection still covers buffer offsets 10-30
5. Resize to width 10 (aggressive wrap)
6. Assert: selection unchanged in buffer space
```

### Workflow 7: The Trust Test (Automated, 1000 iterations)

```
for trial := 0; trial < 1000; trial++ {
    content := randomValidUTF8(0..500 bytes)
    cursors := randomValidCursors(1..5, len(content))
    clock := NewClock()
    ops := randomOps(5..50)

    apply all ops (with clock advancement between 50-500ms)
    undoAll()

    assert content == original (byte-identical)
    assert cursors == original (all fields match)
}
```

---

## Part 7: Spec Gap Testing

Things the spec doesn't explicitly address but WILL break if unhandled.

### Empty Buffer (Every Operation)

| Operation | Input | Expected |
|-----------|-------|----------|
| character-left | `\|` | `\|` (no-op) |
| character-right | `\|` | `\|` (no-op) |
| delete-left | `\|` | `\|` (no-op) |
| delete-right | `\|` | `\|` (no-op) |
| delete-word-left | `\|` | `\|` (no-op) |
| delete-line | `\|` | `\|` (buffer stays empty) |
| move-line-up | `\|` | `\|` (no-op) |
| select-all | `\|` | `\|` (nothing to select) |
| copy (no sel) | `\|` | clipboard = "" or "\n" (edge case — define and test) |
| newline | `\|` | `\n\|` |
| indent | `\|` | `\t\|` |
| paste "hello" | `\|` | `hello\|` |

### Single-Character Buffer

| Operation | Input | Expected |
|-----------|-------|----------|
| delete-left | `a\|` | `\|` (empty) |
| delete-right | `\|a` | `\|` (empty) |
| delete-line | `\|a` | `\|` (empty) |
| select-all | `\|a` | `[a]` |
| word-left | `a\|` | `\|a` |
| clone-line-down | `\|a` | `\|a\na` |

### Buffer Without Trailing Newline + Clone-Line

```
Initial:  "last line|"  (no \n at end)
clone-line-down → "last line|\nlast line"
```

### Paste Distribution Edge Cases

| Cursors | Clipboard lines | Behavior |
|---------|----------------|----------|
| 3 | 3 lines | Distribute: line i → cursor i |
| 3 | 2 lines | NO distribution: full text at each cursor |
| 3 | 4 lines | NO distribution: full text at each cursor |
| 1 | 3 lines | Normal paste: all inserted at cursor |
| 3 | 1 line | Normal paste: that line at each cursor |

### Viewport Edge Cases

| Condition | Expected |
|-----------|----------|
| Height = 0 | No visible rows. Render returns empty string. Must not panic. |
| Width = 0 | Must not panic. Degenerate wrap (every char its own row). |
| Width = 1 | Each char wraps independently. Tab = 1 cell. Must not panic. |

---

## Part 8: Acceptance Criteria

Falsifiable per-section completion statements.

### Navigation is Correct

ALL 14 movement commands produce spec-defined position for all tested scenarios:
- Boundary no-ops (left at 0, right at end) ✓
- line-start toggle cycle (3 states) ✓
- ASCII-only word boundaries ✓
- DesiredCol preservation across variable-length lines ✓
- Selection collapse from both directions ✓
- Page-up/down with overlap ✓

**Measured by:** `TestSpec_Navigation` — minimum 70 entries, all green.

### Editing is Correct

ALL 14 editing commands produce byte-identical expected output:
- Line-join (delete-left/right at boundaries) ✓
- Auto-indent (copies from current line, not stale) ✓
- Move-line boundary no-ops ✓
- Clone-line with/without trailing newline ✓
- Multi-line indent/outdent ✓

**Measured by:** `TestSpec_Editing` — minimum 60 entries, all green.

### Multi-Cursor is Correct

- Reverse-offset produces correct result (P6 + concrete scenarios) ✓
- Merge postcondition after every operation ✓
- add-above/below clamping ✓
- Escape semantics ✓
- Block operation unification ✓

**Measured by:** `TestSpec_MultiCursor` — minimum 30 entries + P6 property (1000 sequences).

### Undo/Redo is Correct

- Trust test: 1000 random sequences (P3) ✓
- Redo invalidation: all tested sequences (P4) ✓
- Coalescing boundary precision (timing, breaks) ✓
- Cursor restoration exact ✓
- Multi-cursor = single undo group ✓

**Measured by:** `TestSpec_Undo*` + trust test property loop.

### Display Pipeline is Correct

- Round-trip invariants (P5) ✓
- Tab position-dependent expansion ✓
- CJK double-width ✓
- Hidden delimiter skipping ✓
- Soft-wrap boundary correctness ✓

**Measured by:** `TestCoords_*` round-trips + scenario tables. Golden tests deferred to Phase 6+.

---

## Part 9: Infrastructure & File Layout

### Test Helper Package

```
internal/editortest/
  notation.go     # ParseState/FormatState — cursor notation DSL
  clock.go        # Deterministic Clock (value type)
  golden.go       # Golden file comparison
  diff.go         # Unified diff output
```

Depends ONLY on standard library. MUST NOT import `pkg/editor/*`.

### Test File Locations

| Package | Test file | Properties covered |
|---------|-----------|-------------------|
| `internal/editortest/` | `*_test.go` | Self-validation (round-trips) |
| `pkg/editor/buffer/` | `buffer_test.go` | P2, P7 + editing scenarios |
| `pkg/editor/cursor/` | `cursor_test.go` | P1, P8 + merge/adjust |
| `pkg/editor/history/` | `history_test.go` | P3, P4 + coalescing |
| `pkg/editor/coords/` | `coords_test.go` | P5 + pipeline scenarios |
| `pkg/editor/display/` | `display_test.go` | P5 integration + tab/CJK/wrap |
| `pkg/editor/keybind/` | `resolver_test.go` | Chord + collision detection |
| `pkg/command/` | `registry_test.go` | Correctness |
| `pkg/ui/components/editor/` | `editor_test.go` | Integration workflows |

### State Notation

```
|        = cursor (no selection)
[text]   = forward selection (anchor at [, position at ])
]text[   = backward selection (position at ], anchor at [)
\|, \[, \] = literal characters
```

### Deterministic Clock

```go
type Clock struct { now time.Time }
func NewClock() Clock
func (c Clock) Now() time.Time
func (c Clock) Advance(d time.Duration) Clock
```

History accepts `func() time.Time`. Tests inject `clock.Now`.

---

## Part 10: What Is NOT Tested (Risk Acceptance)

| What | Why not tested | Mitigation |
|------|---------------|------------|
| Buffer snapshot immutability | Go value semantics enforce this for value types. A test cannot add safety the type system already provides. | If a pointer field is added, code review catches it. |
| Internal data structure layout | Tests assert observable behavior, not HOW it's achieved. | Property tests P2, P3, P7 catch any internal corruption through behavioral symptoms. |
| ANSI escape sequences | Unstable during development. Deferred to Phase 6+. | P5 (coordinate correctness) catches spatial bugs; visual styling deferred. |
| Concurrent Model access | Bubble Tea is single-threaded. No concurrent mutation possible under normal operation. | Architecture violation, not testable at unit level. |
| Performance | Correctness first. Benchmarks added post-feature-complete. | Large file slowness is UX issue, not correctness risk. |
| File I/O errors | Tested at workspace level (WP13). Buffer never does I/O. | Covered by integration tests. |

---

## Verification Commands

```bash
# All editor package tests
go test ./pkg/editor/... ./pkg/command/... ./internal/editortest/... -v

# Property tests with extended fuzzing (CI)
go test ./pkg/editor/buffer/ -fuzz FuzzBufferUTF8Preservation -fuzztime 60s
go test ./pkg/editor/cursor/ -fuzz FuzzCursorBounds -fuzztime 30s
go test ./pkg/editor/history/ -fuzz FuzzUndoCompleteness -fuzztime 60s
go test ./pkg/editor/coords/ -fuzz FuzzCoordRoundTrip -fuzztime 60s

# Integration workflows
go test ./pkg/ui/components/editor/ -run TestWorkflow -v

# Trust test (1000 random sequences)
go test ./pkg/editor/history/ -run TestTrustTest -count=1

# Full verification
go test ./... && go build ./... && go vet ./...
```
