# Fix rust.settle predicate race in parity harness (scenario 01-open-file)

**Status:** open
**Priority:** medium — the parity gate (`make parity`) flakes intermittently, producing false-negative failures. It blocks confidence in screen-parity assertions but does not affect the editor itself or developer workflow outside the harness.

---

### Symptom

`make parity` (specifically `capture.sh rust 01-open-file` followed by `assert.sh`) intermittently fails the gate:

```
FAIL: rust bottom content row (line 33) ends 'sample.md ──╯'
```

The captured Rust screen shows a plain border row at the bottom instead of the breadcrumb with the file name. The failure is consistent under rapid back-to-back automated runs but may pass during slower manual runs.

### Root cause

`scripts/parity/scenarios/01-open-file/rust.settle` contains the predicate `╭Files`, which matches the Explorer pane's top-left border corner. That border paints on the first frame after the `C-b` key (sent by `rust.keys`) opens the Explorer.

However, the bottom-border breadcrumb — rendered by `breadcrumb::overlay` and spliced onto the center pane's border row — depends on `App`'s workspace root, which is populated asynchronously by the `^x`-triggered directory read (`Msg::DirLoaded`). That arrives one frame after the Explorer border.

The settle predicate fires on frame N (Explorer border visible), but the capture happens before frame N+1 (breadcrumb rendered). The `assert.sh` gate then checks for the file name on the bottom border row and finds only the plain border.

### Scope

- `scripts/parity/scenarios/01-open-file/rust.settle` — the settle predicate file (currently contains `╭Files`)
- `scripts/parity/capture.sh` — reads the settle file and calls `wait_for_pane`; no changes needed
- `scripts/parity/lib.sh` — `wait_for_pane` implementation; no changes needed
- `scripts/parity/assert.sh` — the gate that detects the symptom (lines 88-95, checks for `sample.md ──╯` on the bottom content row); no changes needed
- Rust-side breadcrumb rendering (`breadcrumb::overlay`, `Msg::DirLoaded`) — the async behavior is correct; the harness predicate is what needs fixing

### Acceptance criteria

- [ ] `rust.settle` contains a predicate that only appears after the breadcrumb has fully rendered (e.g., the fixture file name `sample.md` on the bottom border row, or the breadcrumb's right corner `╯` preceded by the file name)
- [ ] `make parity` passes deterministically across 10 consecutive runs without manual delays between captures
- [ ] The settle predicate is specific enough that it does not match before the keys take effect (the existing enforcement in `capture.sh` lines 97-106 already requires this)
- [ ] Other scenarios with keys files get their settle predicates reviewed for the same class of race (if any exist)

### Notes

- **Pre-existing.** Reproduces against the untouched `capture.sh` at the base commit of the current branch; unrelated to WP1's `capture.sh` fixture-parameter change.
- **Go side has no equivalent problem.** The Go `go.settle` file does not exist for this scenario, and the Go side either renders the breadcrumb synchronously or the timing works out differently. The `go.keys` file is empty (no keys sent), so the settle enforcement does not apply.
- **The fix is one-line.** Replace the content of `rust.settle` with a grep pattern matching the breadcrumb text. A suitable candidate: `sample\.md.*──╯` or similar, matching what `assert.sh` itself checks for on the bottom row.
- **Discovered incidentally** during WP1 (glyph-grid parity plan) development; the race was already present in the codebase.
