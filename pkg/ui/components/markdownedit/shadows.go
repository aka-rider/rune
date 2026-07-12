// Package markdownedit: this file corrals the PURE re-type shadow methods —
// wrappers forced by value-embedding textedit.Model whose entire body is
// "call the embedded method, reassign, return m" with no side effect of
// markdownedit's own. A shadow belongs here IFF it is exactly that shape
// (m.Model = m.Model.X(...); return m, or the multi-return equivalent). The
// moment a wrapper needs to run image reconciliation, afterContentChange, or
// any other markdownedit-specific bookkeeping, it is NOT a pure shadow and
// belongs in markdownedit.go next to the logic it coordinates (see
// SetRect/SetContent/ApplyInverse/Reapply/ReplaceRange there for examples of
// wrappers that do NOT belong here).
package markdownedit

import (
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// SetFocused shadows textedit.Model.SetFocused to return markdownedit.Model.
func (m Model) SetFocused(f bool) Model {
	m.Model = m.Model.SetFocused(f)
	return m
}

// SetReadOnly shadows textedit.Model.SetReadOnly to return markdownedit.Model.
func (m Model) SetReadOnly(ro bool) Model {
	m.Model = m.Model.SetReadOnly(ro)
	return m
}

// GotoBottom shadows textedit.Model.GotoBottom to return markdownedit.Model.
func (m Model) GotoBottom() Model {
	m.Model = m.Model.GotoBottom()
	return m
}

// DrainEdits shadows textedit.Model.DrainEdits to return markdownedit.Model.
func (m Model) DrainEdits() (Model, []buffer.AppliedEdit) {
	var edits []buffer.AppliedEdit
	m.Model, edits = m.Model.DrainEdits()
	return m, edits
}

// SetCursors shadows textedit.Model.SetCursors to return markdownedit.Model.
func (m Model) SetCursors(cs []cursor.Cursor) Model {
	m.Model = m.Model.SetCursors(cs)
	return m
}

// SetSearchQuery shadows textedit.Model.SetSearchQuery to return markdownedit.Model.
func (m Model) SetSearchQuery(query string, caseInsensitive bool) Model {
	m.Model = m.Model.SetSearchQuery(query, caseInsensitive)
	return m
}

// FindNext shadows textedit.Model.FindNext to return markdownedit.Model.
func (m Model) FindNext() Model {
	m.Model = m.Model.FindNext()
	return m
}

// FindPrev shadows textedit.Model.FindPrev to return markdownedit.Model.
func (m Model) FindPrev() Model {
	m.Model = m.Model.FindPrev()
	return m
}

// ClearSearch shadows textedit.Model.ClearSearch to return markdownedit.Model.
func (m Model) ClearSearch() Model {
	m.Model = m.Model.ClearSearch()
	return m
}
