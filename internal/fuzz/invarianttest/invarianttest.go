// Package invarianttest is the per-package test-helper chokepoint for
// Phase 3 of the QA-rehaul plan: it lets an ordinary `go test` in any
// component package assert the SAME invariants the fuzz driver checks after
// every settled message, against a standalone component Model under test —
// no fuzz driver, no workspace, no session plumbing required.
//
// Import-cycle constraint (critic-verified): invarianttest imports the
// component packages (textedit, markdownedit, opentabs, footer, and
// pkg/ui/pages/workspace for CheckWorkspace), and rune/internal/fuzz/snapshot
// itself imports textedit/markdownedit/opentabs/footer. So these
// model-taking helpers are callable only from an EXTERNAL test file
// (`package X_test`) of the component they check — a package-internal test
// file (`package textedit`) importing invarianttest would close a cycle
// back on itself (invarianttest imports textedit). This mirrors
// internal/fuzz/harness's constraint one level up (workspace's own tests).
//
// Two consequences baked into the design (see the plan, Phase 3):
//   - Workspace's internal `settle` helper (workspace_test.go) does NOT use
//     invarianttest; it calls session.Check(m.FuzzInspect()) directly —
//     session/snapshot do not import workspace, so no cycle.
//   - A component's existing INTERNAL tests that want CheckTextedit/
//     CheckFooter/etc must move to (or gain a new) external test file;
//     internal tests that can't move stay on plain behavior asserts.
package invarianttest

import (
	"testing"

	"rune/internal/fuzz/session"
	"rune/internal/fuzz/snapshot"
	footerchk "rune/internal/fuzz/ui/footer"
	opentabschk "rune/internal/fuzz/ui/opentabs"
	texteditchk "rune/internal/fuzz/ui/textedit"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/pages/workspace"
)

// CheckWorkspace runs the full session.Check invariant sweep (SHADOW,
// SAVE-VERBATIM, EXT-NOCLOBBER, DIRTYDOCS-GROUND-TRUTH, cell/cursor/
// display/layout/guard invariants, ...) against m's current FuzzInspect()
// snapshot. Fails the test via t.Fatalf with the violated InvariantID +
// message.
func CheckWorkspace(t *testing.T, m workspace.Model) {
	t.Helper()
	if v := session.Check(m.FuzzInspect()); v != nil {
		t.Fatalf("invariant %s: %s", v.InvariantID, v.Message)
	}
}

// CheckWorkspaceTransition runs the L1 transition invariants (G2, ...)
// against (prev, msg, next) — the same session.CheckTransition the fuzz
// driver calls after every settled message. Fails the test via t.Fatalf on
// the first violation found.
func CheckWorkspaceTransition(t *testing.T, prev workspace.Model, msg any, next workspace.Model) {
	t.Helper()
	if vs := session.CheckTransition(prev.FuzzInspect(), msg, next.FuzzInspect()); len(vs) > 0 {
		v := vs[0]
		t.Fatalf("invariant %s: %s", v.InvariantID, v.Message)
	}
}

// CheckTextedit runs the textedit-domain L0 invariants (R1-R9 cell layout,
// C1-C3 cursor geometry, M1-M2 presence, B1 line count, S1 selection
// coverage) against a standalone textedit.Model, via the exact same
// snapshot.FromTextedit field mapping workspace_fuzz.go's FuzzInspect uses.
func CheckTextedit(t *testing.T, m textedit.Model) {
	t.Helper()
	if v := texteditchk.Check(snapshot.FromTextedit(m)); v != nil {
		t.Fatalf("invariant %s: %s", v.InvariantID, v.Message)
	}
}

// CheckMarkdownedit is CheckTextedit for a standalone markdownedit.Model —
// markdownedit's Fuzz* accessors forward to its embedded textedit.Model, so
// this runs the same textedit-domain checks via snapshot.FromMarkdownedit.
func CheckMarkdownedit(t *testing.T, m markdownedit.Model) {
	t.Helper()
	if v := texteditchk.Check(snapshot.FromMarkdownedit(m)); v != nil {
		t.Fatalf("invariant %s: %s", v.InvariantID, v.Message)
	}
}

// CheckOpenTabs runs the opentabs-domain L0 invariants (T1 no duplicate tab
// paths, T2 active index in range, TAB-SET exactly one active flag) against
// a standalone opentabs.Model, via snapshot.FromOpenTabs.
func CheckOpenTabs(t *testing.T, m opentabs.Model) {
	t.Helper()
	if v := opentabschk.Check(snapshot.FromOpenTabs(m)); v != nil {
		t.Fatalf("invariant %s: %s", v.InvariantID, v.Message)
	}
}

// CheckFooter runs the footer-domain L0 invariant (GUARD-SYNC: GuardVisible
// iff GuardOptionCount > 0) against a standalone footer.Model, via
// snapshot.FromFooter.
func CheckFooter(t *testing.T, m footer.Model) {
	t.Helper()
	if v := footerchk.Check(snapshot.FromFooter(m)); v != nil {
		t.Fatalf("invariant %s: %s", v.InvariantID, v.Message)
	}
}
