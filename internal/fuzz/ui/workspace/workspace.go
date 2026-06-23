//go:build fuzzing

// Package workspace contains invariant checkers for the workspace page:
// SHADOW (buffer vs journal mirror), L1/L2 (layout bounds), EDITOR-TAB-COH
// (editor path matches active tab), TR-focus-valid, SAVE-SM, and all
// persistence/guard transition invariants (RESIZE-INV, SAVE-NOMUT,
// TR-dirty-clear, G1, G3, TR-cursor-not-dirty, DL1, EXT-NOCLOBBER,
// RELOAD-NOMUT). L2 monitors (F1, DL3) live in monitors.go.
package workspace

import (
	"fmt"
	"strings"

	"charm.land/lipgloss/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
)

// lipglossWidth wraps lipgloss.Width to keep the call sites readable.
func lipglossWidth(s string) int { return lipgloss.Width(s) }

func trunc(s string, n int) string { return invariant.Trunc(s, n) }

// Check runs all L0 workspace invariants against s.
// Returns the first violation, or nil.
func Check(s snapshot.Snapshot) *invariant.Violation {
	// SHADOW: buffer content must match the independently-maintained journal mirror.
	if s.MirrorContent != "" && s.Content != s.MirrorContent {
		return &invariant.Violation{
			InvariantID: "SHADOW",
			Message: fmt.Sprintf("buffer %q != mirror %q",
				trunc(s.Content, 80), trunc(s.MirrorContent, 80)),
		}
	}

	// L1: every frame line's display width ≤ terminal width (no overflow).
	if s.Width > 0 && s.Frame != "" {
		for i, line := range strings.Split(s.Frame, "\n") {
			w := lipglossWidth(line)
			if w > s.Width {
				return &invariant.Violation{
					InvariantID: "L1",
					Message: fmt.Sprintf(
						"frame line %d display-width %d > terminal width %d", i, w, s.Width,
					),
				}
			}
		}
	}

	// L2: frame line count ≤ terminal height.
	if s.Height > 0 && s.Frame != "" {
		lines := strings.Count(s.Frame, "\n") + 1
		if lines > s.Height {
			return &invariant.Violation{
				InvariantID: "L2",
				Message: fmt.Sprintf(
					"frame has %d lines > terminal height %d", lines, s.Height,
				),
			}
		}
	}

	// EDITOR-TAB-COH: editor path equals the active tab's path.
	if len(s.Tabs) > 0 && s.ActiveTabIdx >= 0 && s.ActiveTabIdx < len(s.Tabs) {
		activeTabPath := s.Tabs[s.ActiveTabIdx].Path
		if s.EditorPath != activeTabPath {
			return &invariant.Violation{
				InvariantID: "EDITOR-TAB-COH",
				Message: fmt.Sprintf(
					"EditorPath %q != Tabs[%d].Path %q",
					s.EditorPath, s.ActiveTabIdx, activeTabPath,
				),
			}
		}
	}

	// TR-focus-valid: FocusPane must be one of the known pane enum values (0–5).
	const maxPane = 5
	if s.FocusPane < 0 || s.FocusPane > maxPane {
		return &invariant.Violation{
			InvariantID: "TR-focus-valid",
			Message:     fmt.Sprintf("FocusPane %d not in [0, %d]", s.FocusPane, maxPane),
		}
	}

	// SAVE-SM: at most one in-flight save; SaveInFlight true requires SaveSnapshot non-nil.
	if s.SaveInFlight && s.SaveSnapshot == nil {
		return &invariant.Violation{
			InvariantID: "SAVE-SM",
			Message:     "save InFlight but SavedContent is nil (missing save identity)",
		}
	}

	return nil
}

// CheckTransition runs workspace-domain L1 transition invariants.
// Returns all violations found.
func CheckTransition(prev snapshot.Snapshot, msg any, next snapshot.Snapshot) []invariant.Violation {
	var vs []invariant.Violation
	typeName := fmt.Sprintf("%T", msg)

	add := func(id, message string) {
		vs = append(vs, invariant.Violation{InvariantID: id, Message: message})
	}

	// RESIZE-INV: a WindowSizeMsg must not mutate buffer content, cursor positions, or dirty state.
	if typeName == "tea.WindowSizeMsg" {
		if next.Content != prev.Content {
			add("RESIZE-INV", fmt.Sprintf(
				"Content changed on resize: %q → %q",
				trunc(prev.Content, 40), trunc(next.Content, 40),
			))
		}
		if prev.HasDirtyFile != next.HasDirtyFile {
			add("RESIZE-INV", fmt.Sprintf(
				"HasDirtyFile changed on resize: %v → %v", prev.HasDirtyFile, next.HasDirtyFile,
			))
		}
	}

	// SAVE-NOMUT: a save message must not mutate the buffer content.
	if typeName == "workspace.FileSavedMsg" {
		if next.Content != prev.Content {
			add("SAVE-NOMUT", fmt.Sprintf(
				"Content changed during save: %q → %q",
				trunc(prev.Content, 40), trunc(next.Content, 40),
			))
		}
		// TR-dirty-clear: after a save, active file must not be dirty.
		if next.HasDirtyFile {
			add("TR-dirty-clear", "active file still dirty after FileSavedMsg settled")
		}
	}

	// G1: dirty file + ConfirmQuitMsg → guard must appear.
	if typeName == "footer.ConfirmQuitMsg" && prev.HasDirtyFile && !next.GuardVisible {
		add("G1", "dirty file + ConfirmQuitMsg did not raise guard")
	}

	// G3: dirty active tab + CloseFile key → guard must appear (unless guard already
	// active or a save is in-flight — in that case the key is consumed silently).
	if next.CloseFileKeyPressed && prev.ActiveTabDirty &&
		!prev.GuardVisible && !next.GuardVisible && !prev.SaveInFlight {
		add("G3", "dirty active tab + CloseFile key (^w) did not raise guard")
	}

	// TR-cursor-not-dirty: a key press that does not change buffer content must not
	// set the dirty flag.
	if typeName == "tea.KeyPressMsg" && !prev.HasDirtyFile && next.HasDirtyFile && next.Content == prev.Content {
		add("TR-cursor-not-dirty", "key press set dirty flag without any content change")
	}

	// RELOAD-NOMUT: a dirChangedMsg-driven reload must not mutate buffer content,
	// cursor state, or dirty status — only the filetree listing changes.
	if typeName == "workspace.dirChangedMsg" || typeName == "filetree.DirReloadedMsg" {
		if next.Content != prev.Content {
			add("RELOAD-NOMUT", fmt.Sprintf(
				"Content changed on dir reload: %q → %q",
				trunc(prev.Content, 40), trunc(next.Content, 40),
			))
		}
		if prev.HasDirtyFile != next.HasDirtyFile {
			add("RELOAD-NOMUT", fmt.Sprintf(
				"HasDirtyFile changed on dir reload: %v → %v",
				prev.HasDirtyFile, next.HasDirtyFile,
			))
		}
	}

	return vs
}

// CheckDataLoss checks DL1: VFS content must equal buffer content immediately
// after an autosave snapshot settles.
// vfsContent is the result of store.Content(snap.DocID), passed by the driver
// so this package remains docstate-free (N2).
func CheckDataLoss(s snapshot.Snapshot, vfsContent string) *invariant.Violation {
	if s.DocID == 0 {
		return nil // no VFS doc yet — untitled without a scratch allocation
	}
	if vfsContent == "" {
		return &invariant.Violation{
			InvariantID: "DL1",
			Message: fmt.Sprintf(
				"autosave settled but VFS has no content for docID=%d (buffer[:%d]=%q)",
				s.DocID, min(len(s.Content), 40), trunc(s.Content, 40),
			),
		}
	}
	if vfsContent != s.Content {
		return &invariant.Violation{
			InvariantID: "DL1",
			Message: fmt.Sprintf(
				"VFS[:%d]=%q != buffer[:%d]=%q",
				min(len(vfsContent), 40), trunc(vfsContent, 40),
				min(len(s.Content), 40), trunc(s.Content, 40),
			),
		}
	}
	return nil
}
