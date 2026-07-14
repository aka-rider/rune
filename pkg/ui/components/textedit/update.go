package textedit

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
)

func (m Model) Init() tea.Cmd { return nil }

// Update handles messages and returns accumulated commands.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	// Reset per-message edit provenance (FuzzLastEdits) unconditionally: only
	// applyOperation repopulates it (below), so any branch that returns
	// without an edit — or without calling applyOperation at all — must not
	// leak a stale prior message's edits into this message's snapshot.
	m.lastEdits = nil
	switch msg := msg.(type) {
	case ClipboardContentMsg:
		if len(msg.ImageData) > 0 {
			// Image paste is a no-op in plain textedit
			return m, nil
		}
		var cmd tea.Cmd
		m, cmd = m.handlePasteContent(msg.Text)
		return m, cmd

	case tea.ClipboardMsg:
		if !m.focused {
			return m, nil
		}
		var cmd tea.Cmd
		m, cmd = m.handlePasteContent(msg.Content)
		return m, cmd

	case tea.PasteMsg:
		if !m.focused {
			return m, nil
		}
		var cmd tea.Cmd
		m, cmd = m.handlePasteContent(msg.Content)
		return m, cmd

	case tea.WindowSizeMsg:
		// Window size is handled by the parent via SetRect; children do NOT
		// handle tea.WindowSizeMsg directly (per CLAUDE.md component contracts).
		return m, nil

	case tea.KeyPressMsg:
		return m.updateKeys(msg, &cmds)
	}
	return m, tea.Batch(cmds...)
}

func (m Model) updateKeys(msg tea.KeyPressMsg, cmds *[]tea.Cmd) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}

	// PrimaryAction: Enter key routes directly to edit.newline (no resolver
	// binding). "enter" is filtered out of keymap.CommandBindings (the only
	// production binding producer), so a registered edit.newline's Result.Err
	// is always nil here — the resolver fallthrough this used to gate on
	// res.Err was already dead; this is now an unconditional return.
	if msg.Code == tea.KeyEnter && msg.Mod == 0 {
		if m.singleLine {
			// No-op in single-line mode
			return m, nil
		}
		res := m.registry.Execute("edit.newline", m.basicCtx())
		m = m.applyResult(res)
		return m, tea.Batch(*cmds...)
	}

	// Cancel: Escape key routes to multicursor.escape (no resolver binding).
	// "esc" is filtered out of keymap.CommandBindings the same way "enter" is
	// (see above), so falling through to the resolver on a no-op escape can
	// never match any chord — provably inert, hence the unconditional return.
	if msg.Code == tea.KeyEscape && msg.Mod == 0 {
		res := m.registry.Execute("multicursor.escape", m.basicCtx())
		m = m.applyResult(res)
		return m, tea.Batch(*cmds...)
	}

	// Resolve via keybind resolver for all other keys
	chord := keybind.ChordFromKeyMsg(msg)
	hasSel := false
	for _, c := range m.cursors.All() {
		if c.HasSelection() {
			hasSel = true
			break
		}
	}
	resCtx := keybind.ResolverContext{
		EditorFocused:  true,
		HasSelection:   hasSel,
		HasMultiCursor: m.cursors.IsMulti(),
		ReadOnly:       m.readOnly,
	}
	resResult := m.resolver.Resolve(chord, resCtx)
	switch resResult.Kind {
	case keybind.ResultFound:
		contentHeight := m.contentHeight()
		topRow := m.viewport.TopRow
		scrollCol := m.viewport.ScrollCol
		// wrapRowCount is wrap-space (m.wrapSnap), matching SyntaxToWrap /
		// WrapToSyntax / WrapByteCol below — NOT m.snapshot.TotalRows, which is
		// display-space and can be larger once table/image row expansion runs.
		wrapRowCount := m.wrapSnap.TotalRows
		res := m.registry.Execute(resResult.Command, command.CommandContext{
			Buffer:         m.buf,
			Cursors:        m.cursors,
			FilePath:       "", // textedit doesn't own file path
			NewRequestID:   func() string { return "" },
			HashContent:    func(string) string { return "" },
			BufferToSyntax: m.syntaxSnap.BufferToSyntax,
			SyntaxToBuffer: m.syntaxSnap.SyntaxToBuffer,
			SyntaxToWrap:   m.wrapSnap.SyntaxToWrap,
			WrapToSyntax:   m.wrapSnap.WrapToSyntax,
			WrapVisualCol:  m.wrapSnap.VisualCol,
			WrapByteCol:    m.wrapSnap.ByteColFromVisual,
			ViewportBounds: func() (int, int) { return topRow, topRow + contentHeight },
			ScrollCol:      func() int { return scrollCol },
			ViewportHeight: func() int { return contentHeight },
			WrapRowCount:   func() int { return wrapRowCount },
			ReadOnly:       m.readOnly,
		})
		if res.Cmd != nil {
			*cmds = append(*cmds, res.Cmd)
		}
		m = m.applyResult(res)
	case keybind.ResultNoMatch:
		text := msg.Text
		if text == "" && msg.Mod == 0 {
			code := msg.BaseCode
			if code == 0 {
				code = msg.Code
			}
			if isPrintableChar(code) {
				text = string(code)
			}
		}
		if text != "" {
			// Guard: read-only blocks printable char insertion
			if m.readOnly {
				return m, tea.Batch(*cmds...)
			}
			res := m.registry.Execute("edit.insert-character", command.CommandContext{
				Buffer:  m.buf,
				Cursors: m.cursors,
				Args:    map[string]any{"char": text},
			})
			if res.Operation.Kind != command.OperationNone {
				m = m.applyResult(res)
			}
		}
	}

	return m, tea.Batch(*cmds...)
}
