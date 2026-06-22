//go:build fuzzing

// Package session is the invariant aggregator for the fuzz driver. It
// composes all per-domain checker packages (textedit, display, opentabs,
// footer, filetree, workspace) into single Check / CheckTransition /
// CheckDataLoss / ObserveMonitors entry points that the driver calls after
// every settled message.
//
// Import graph (no cycles):
//
//	session → ui/textedit, ui/opentabs, ui/footer, ui/filetree,
//	          editor/display, ui/workspace → snapshot → (components)
//	session → invariant → (stdlib only)
package session

import (
	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
	displaychk "rune/internal/fuzz/editor/display"
	footerchk "rune/internal/fuzz/ui/footer"
	filetreechk "rune/internal/fuzz/ui/filetree"
	opentabschk "rune/internal/fuzz/ui/opentabs"
	texteditchk "rune/internal/fuzz/ui/textedit"
	workspacechk "rune/internal/fuzz/ui/workspace"
)

// Check runs all L0 per-snapshot invariants.
// Returns the first violation found (first-wins order: textedit → display →
// opentabs → footer → filetree → workspace).
func Check(s snapshot.Snapshot) *invariant.Violation {
	if v := texteditchk.Check(s); v != nil {
		return v
	}
	if v := displaychk.Check(s); v != nil {
		return v
	}
	if v := opentabschk.Check(s); v != nil {
		return v
	}
	if v := footerchk.Check(s); v != nil {
		return v
	}
	if v := filetreechk.Check(s); v != nil {
		return v
	}
	if v := workspacechk.Check(s); v != nil {
		return v
	}
	return nil
}

// CheckTransition runs all L1 transition invariants against (prev, msg, next).
// Returns all violations found; the driver uses the first one.
func CheckTransition(prev snapshot.Snapshot, msg any, next snapshot.Snapshot) []invariant.Violation {
	var vs []invariant.Violation
	vs = append(vs, texteditchk.CheckTransition(prev, msg, next)...)
	vs = append(vs, footerchk.CheckTransition(prev, msg, next)...)
	vs = append(vs, workspacechk.CheckTransition(prev, msg, next)...)
	return vs
}

// CheckDataLoss checks DL1 (VFS content vs. buffer) immediately after an
// autosave snapshot settles. vfsContent is provided by the driver (so the
// checker packages stay docstate-free).
func CheckDataLoss(s snapshot.Snapshot, vfsContent string) *invariant.Violation {
	return workspacechk.CheckDataLoss(s, vfsContent)
}

// NewMonitors creates a fresh set of all L2 stateful monitors for one Run
// call. Call Reset on each monitor at the start of each shrink replay.
func NewMonitors() []invariant.Monitor {
	return workspacechk.NewMonitors()
}

// ObserveMonitors fans out to all monitors. Returns all violations from the
// first monitor that fires (first-wins, matching driver semantics).
func ObserveMonitors(monitors []invariant.Monitor, prev snapshot.Snapshot, msg tea.Msg, next snapshot.Snapshot) []invariant.Violation {
	for _, mon := range monitors {
		if vs := mon.Observe(prev, msg, next); len(vs) > 0 {
			return vs
		}
	}
	return nil
}
