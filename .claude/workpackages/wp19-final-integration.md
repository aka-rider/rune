# WP19 — Final Integration & Polish

## Scope

Cross-cutting verification, file size compliance, collision tests, smoke testing

## Dependencies

- ALL previous work packages (WP0–WP18)

## Deliverables

### Keybinding Collision Test

Add test that scans all physical key strings in `pkg/ui/keymap/keymap.go` and fails on duplicates:

```go
func TestNoKeybindingCollisions(t *testing.T) {
    // Extract all key strings from all Bindings fields
    // Assert each key string appears in exactly one binding
    // Exception: keys with explicit context predicates that prevent overlap
}
```

### 500 LoC File Size Compliance

Verify and enforce:
```bash
find . -name '*.go' -not -path './vendor/*' -exec wc -l {} + | awk '$1 > 500 {print "OVER 500:", $0; exit 1}'
```

If any files exceed 500 lines, decompose them:
- Split large command files by category (navigation, editing, multi-cursor)
- Split large test files by spec section
- Extract helpers into internal sub-functions

### Full Build & Test Suite

```bash
go test ./...
go build ./...
go vet ./...
```

All must pass with zero failures.

### Spec Coverage Audit

Verify every row in the spec action tables has at least one test:

| Spec Section | Expected Test Count |
|---|---|
| Navigation (14 commands × 4+ cases) | ≥56 |
| Selection (11 commands × 3+ cases) | ≥33 |
| Editing (10 commands × 4+ cases) | ≥40 |
| Multi-Cursor (3 commands + algorithm) | ≥15 |
| Clipboard (3 commands × 3+ cases) | ≥9 |
| Undo/Redo (coalescing + inversion) | ≥12 |
| Mouse (7 actions × 2+ cases) | ≥14 |
| Find/Replace (6 commands × 2+ cases) | ≥12 |
| Keybind resolver | ≥10 |
| Buffer invariants + fuzz | ≥10 + fuzz |
| Cursor invariants | ≥8 |
| Display pipeline | ≥10 |
| Coordinate round-trips | ≥6 |

### Manual Smoke Test Checklist

Run the app and verify:
- [ ] Open a markdown file from file tree
- [ ] Type text — appears at cursor
- [ ] Delete with backspace — works at mid-line and line boundaries
- [ ] Select with Shift+arrows — visual selection appears
- [ ] Multi-cursor with Alt+Cmd+↓ — multiple cursors visible
- [ ] Type with multi-cursor — text inserted at all positions
- [ ] Move line up/down with Alt+↑/↓
- [ ] Clone line with Alt+Shift+↓
- [ ] Undo (Cmd+Z) — restores previous state
- [ ] Redo (Cmd+Shift+Z) — re-applies
- [ ] Switch focus to file tree (Tab) — editor stops accepting input
- [ ] Switch back — editor resumes
- [ ] Edit file, try switching to another file — dirty guard appears
- [ ] Markdown headings render styled when cursor is away
- [ ] Move cursor onto heading — raw `#` syntax visible
- [ ] Bold text renders/reveals correctly
- [ ] Code fence renders with syntax highlighting
- [ ] Soft-wrap works for long lines
- [ ] Page up/down scrolls correctly
- [ ] Mouse click positions cursor
- [ ] Mouse scroll works
- [ ] No panic, no data loss, no silent errors

### Documentation

- Update CLAUDE.md if any new patterns were established
- Document new dependencies introduced (goldmark, etc.)
- Ensure `editor-spec.md` is referenced but not modified

## Constraints

- This WP produces no new features — only verification and compliance
- Any failures discovered here create issues to be fixed in the relevant WP
- Under 500 LoC per file (enforcement, not just checking)

## QA Gates

Final gates — if these pass, the editor is shippable.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | ALL properties P1-P8 pass against 1000 random operation sequences (Monte Carlo trust test at full integration level) | Any subtle invariant violation that individual WP tests missed due to simpler contexts |
| 2 | All 7 integration workflows pass (write paragraph, multi-cursor rename, reorganize, clipboard, recover from mistake, resize mid-edit, trust test) | Real user workflows broken despite individual operations passing |
| 3 | Zero keybinding collisions (automated scan of all binding key strings) | Ambiguous key dispatch → unpredictable behavior for user |
| 4 | All spec-gap scenarios pass (every operation on empty buffer, single-char buffer, no-trailing-newline + clone) | Edge cases that exist in real usage cause panics or data loss |
| 5 | `go test ./... && go build ./... && go vet ./...` with zero failures | Any compilation or test regression across the full project |

**Testing approach:** Gate 1 via dedicated Monte Carlo test. Gate 2 via integration test file. Gate 3 via static analysis test. Gates 4-5 via CI commands.

## Verification

```bash
go test ./... && go build ./... && go vet ./...
find . -name '*.go' -not -path './vendor/*' -exec wc -l {} + | awk '$1 > 500 {print "OVER 500:", $0; exit 1}'
go test ./pkg/ui/keymap/ -run TestNoKeybindingCollisions -v
```
