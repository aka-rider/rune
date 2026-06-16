# Composite / stateful invariants — design addendum to plan-fuzzy-testing.md

## Context

The user asked: how do you specify and check invariants that are only valid *given a prior state*?
Canonical example: if a file is dirty AND the user triggers quit → the save/discard/cancel guard MUST
appear in the next frame. A single-snapshot checker cannot see this — it has no memory of "quit was
just triggered."

Codebase investigation confirmed:
- Dirty flag infrastructure exists: `opentabs.Tab.Dirty`, `MarkDirty`/`MarkClean` methods
  (`pkg/ui/components/opentabs/opentabs.go:19–162`)
- Guard UI framework exists: `footer.GuardKind`, `SetGuard()`, `DataLossGuardResponseMsg`
  (`pkg/ui/components/footer/footer.go:23–143`)
- **Neither is wired**: workspace.go `ConfirmQuitMsg` handler (line 1038) quits unconditionally;
  `MarkDirty` is never called from workspace logic. The guard is a known TODO, not a live feature.
- Footer test comment at `footer_test.go:12–14` explicitly documents: "Dirty-state handling (guard
  prompt) is the workspace's responsibility, not the footer's."

This means G1 below will **trip immediately** once implemented — making it a perfect smoke test
that the fuzzer catches real missing features, not synthetic violations.

---

## Taxonomy: three levels of invariant complexity

| Level | Checked against | API signature | Examples |
|-------|----------------|---------------|---------|
| **L0** snapshot | one model state | `Check(s Snapshot) []Violation` | no-dup-tabs T1, cursor-in-bounds R4, active-tab-in-range T2 |
| **L1** transition | `(prev, msg, next)` triple | `CheckTransition(prev Snapshot, msg tea.Msg, next Snapshot) []Violation` | dirty-exit guard G1, guard-resolve G2 |
| **L2+** monitor | state machine across N steps | `Monitor.Observe(prev, msg, next) []Violation` | liveness F1 (quit must eventually complete), F2 (opened file appears in tabs) |

All existing invariants (RENDER R1–R7, SHADOW, TAB BAR T1–T2, DATA-LOSS) are **L0**.
The guard invariants below are **L1**. Multi-step liveness invariants are **L2+** (Phase 2+).

---

## L1 design: transition invariants

### API

```go
// internal/fuzz/invariant/invariant.go  (extends existing file)

// CheckTransition evaluates invariants that require (prev, msg, next) context.
// Called by the driver after EVERY settled message, alongside the existing Check(next).
func CheckTransition(prev Snapshot, msg tea.Msg, next Snapshot) []Violation
```

The driver already calls `Check(nextSnap)` after each settled message. Extend the call site to:

```go
// driver/driver.go — after each settled message
prevSnap := fuzzInspect(model)
model, cmd = model.Update(msg)
drainCmds(cmd)
nextSnap := fuzzInspect(model)

violations = append(violations, invariant.Check(nextSnap)...)               // L0 (existing)
violations = append(violations, invariant.CheckTransition(prevSnap, msg, nextSnap)...)  // L1 (new)
for _, mon := range session.monitors {                                        // L2+ (new)
    violations = append(violations, mon.Observe(prevSnap, msg, nextSnap)...)
}
```

### How the checker "knows" the expected behavior

You encode the spec as a named function. Each L1 invariant has three parts:

1. **Guard predicate** — when does this check apply? (`msg` type + precondition on `prev`)
2. **Expected postcondition** — what must be true in `next`?
3. **Violation description** — human-readable failure message

```go
func checkDirtyExitGuard(prev Snapshot, msg tea.Msg, next Snapshot) *Violation {
    if _, ok := msg.(footer.ConfirmQuitMsg); !ok { // guard: only on quit confirmation
        return nil
    }
    if !prev.HasDirtyFile { // guard: only when something is unsaved
        return nil
    }
    if next.GuardVisible { // postcondition met
        return nil
    }
    return &Violation{
        InvariantID: "GUARD-G1",
        Desc:        "ConfirmQuit while file dirty: save/discard guard must appear",
        Prev:        prev,
        Msg:         fmt.Sprintf("%T", msg),
        Next:        next,
    }
}
```

The pattern is: guard returns `nil` (skip) if the precondition isn't met; otherwise asserts the
postcondition and returns a `Violation` on failure. This is identical to the QuickCheck/Hypothesis
**command postcondition** pattern, and to the LTL temporal formula `G(dirty ∧ quit → X guard)`.

---

## L2+ design: monitor automata

For sequences longer than one step (liveness, "eventually" properties):

```go
// internal/fuzz/invariant/monitor.go

type Monitor interface {
    // Observe is called after every settled message (same cadence as CheckTransition).
    // Monitors are stateful — they accumulate context across calls.
    // Must be deterministic: no randomness, no I/O.
    Observe(prev Snapshot, msg tea.Msg, next Snapshot) []Violation
    // Reset clears accumulated state. Called by the shrinker before each replay.
    Reset()
}
```

Example — liveness monitor for F1 (quit must complete within N events):

```go
type QuitLivenessMonitor struct {
    pendingSince int  // event count when quit was triggered, -1 if not pending
    eventCount  int
    maxSteps    int  // e.g. 20
}

func (m *QuitLivenessMonitor) Observe(prev, msg, next Snapshot) []Violation {
    m.eventCount++
    if _, ok := msg.(footer.ConfirmQuitMsg); ok && !next.GuardVisible {
        m.pendingSince = m.eventCount
    }
    if m.pendingSince >= 0 && m.eventCount-m.pendingSince > m.maxSteps && !next.AppQuitting {
        return []Violation{{InvariantID: "FLOW-F1", Desc: "app never quit after ConfirmQuit"}}
    }
    if next.AppQuitting {
        m.pendingSince = -1 // resolved
    }
    return nil
}
```

**Shrinking note:** monitors are reset (`mon.Reset()`) before each shrink replay attempt. Since
monitors are deterministic, replay always produces the same violation — no false negatives during
shrinking.

---

## GUARD invariants (L1)

| ID | Trigger condition (prev + msg) | Expected postcondition (next) | Severity |
|----|-------------------------------|-------------------------------|----------|
| G1 | `ConfirmQuitMsg` + `prev.HasDirtyFile == true` | `next.GuardVisible == true` | HIGH |
| G2 | `DataLossGuardResponseMsg{Action: Save\|Discard\|Cancel}` | `next.GuardVisible == false` | HIGH |
| G3 | `DataLossGuardResponseMsg{Action: Save}` | `next.HasDirtyFile == false` (save was effective) | MEDIUM |

G1 will trip immediately on the current codebase (workspace.go:1038 quits unconditionally without
checking dirty state). This validates the invariant machinery end-to-end against a real bug.

---

## Snapshot fields to add for GUARD invariants

`workspace.FuzzInspect()` must expose (in `workspace_fuzz.go`, `//go:build fuzzing`):

```go
type Snapshot struct {
    // ... existing fields (Cells, Cursors, BufferContent, etc.) ...

    // Guard / exit
    HasDirtyFile bool          // any opentabs tab has Dirty==true
    GuardVisible bool          // footer.InGuard() == true
    GuardKind    footer.GuardKind // GuardDirty, GuardMerge, or 0

    // Chord state (for L0 chord invariants and L2 liveness)
    ChordPending bool
    AppQuitting  bool          // tea.Quit was returned (driver sets this flag)
}
```

Derivation:
- `HasDirtyFile`: iterate `m.opentabs` tabs, OR all `Dirty` fields
- `GuardVisible` / `GuardKind`: `m.footer.InGuard()` / `m.footer.GuardKind()`
- `ChordPending`: `m.footer.PendingKey() != ""` (add `PendingKey() string` accessor to footer)

---

## Seeding strategy for composite precondition states

The fuzzer must *reach* the precondition (dirty + quit) to trigger G1. Random fuzzing rarely hits
specific state combinations. Three mechanisms:

1. **LLM seed corpus** (primary): the `f.Add` calls in `FuzzSession` include explicit seeds that
   exercise dirty-exit flows: `["open file", "insert char", "ctrl+c", "ctrl+c"]`.
2. **Precondition-annotated seeds**: each GUARD invariant lists its minimal seed in the invariant
   definition — the fuzzer corpus always starts with it. Format: JSONL event script.
3. **Snapshot-aware event weighting** (Phase 1+): when the driver observes `snap.HasDirtyFile==true`,
   bias the random event generator toward quit-key events. Implement as a `WeightedEventGen` that
   inspects the current snapshot.

---

## Changes to plan-fuzzy-testing.md

Apply these edits to `plan-fuzzy-testing.md`:

### 1. `internal/fuzz/invariant/` component description

Extend the invariant list to distinguish L0/L1/L2+ and add the GUARD invariants:

```
... RENDER R1–R7 on []Cell/DisplaySnapshot (L0); SHADOW oracle (L0); TAB BAR T1–T2 (L0);
GUARD G1–G3 (L1 — transition invariants requiring (prev, msg, next)); MODEL re-checks (L0);
DATA-LOSS (L0, HARD STOP).
L1 invariants require CheckTransition(prev Snapshot, msg tea.Msg, next Snapshot) []Violation —
called by the driver alongside Check(next) after every settled message.
L2+ liveness invariants use long-lived Monitor automata (Reset() before each shrink replay).
```

### 2. Phase 1 invariant list (line ~152)

```
- All RENDER R1–R7 + TAB BAR T1–T2 + GUARD G1–G3 + MODEL re-checks (L0 and L1);
  event-level delta-debug shrinker → minimal keys.jsonl.
- Snapshot-aware event weighting for precondition-sensitive invariants (G1 seed: dirty+quit path).
```

### 3. D3 Decision Log

```
- **D3 Invariants (all 5 families, render-first)**: RENDER R1–R7 (L0, screenshot-provable);
  SHADOW oracle (L0); TAB BAR T1–T2 (L0); GUARD G1–G3 (L1 — transition; requires
  CheckTransition(prev, msg, next) alongside the existing Check(next)); DATA-LOSS (L0, HARD STOP);
  MODEL cheap live re-checks (L0). R2 respects reveal/render hiding.
  L2+ liveness monitors (FLOW F1–F2) deferred to Phase 2.
```

### 4. New D16 entry (Decision Log)

```
- **D16 Invariant levels (L0/L1/L2+)**: L0 = stateless snapshot check; L1 = (prev, msg, next)
  transition check (guard: only when msg type + prev condition met); L2+ = Monitor automaton
  (stateful, Reset() per shrink replay, for liveness/"eventually" properties). CheckTransition
  added to invariant package; driver calls it after every settled message alongside Check().
  Seeding for precondition-gated invariants: explicit LLM seeds + snapshot-aware event weighting.
```

---

## Verification

After implementing:
- `go test -tags fuzzing ./internal/fuzz/invariant/` — unit-test CheckTransition with a hand-crafted
  (dirty prev, ConfirmQuitMsg, no-guard next) triple; assert G1 fires.
- `go test -tags fuzzing ./pkg/ui/pages/workspace/ -run FuzzSession/guard_seed` — replay a seed
  with [insert, ^C, ^C]; confirm G1 fires and the violation artifact is written.
- After wiring workspace.go to call `footer.SetGuard()` on ConfirmQuit+dirty: re-run same seed;
  assert G1 no longer fires (invariant-as-spec drove the feature implementation).
