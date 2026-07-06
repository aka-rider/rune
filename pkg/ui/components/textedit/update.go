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

	// PrimaryAction: Enter key routes directly to edit.newline (no resolver binding)
	if msg.Code == tea.KeyEnter && msg.Mod == 0 {
		if m.singleLine {
			// No-op in single-line mode
			return m, nil
		}
		ctx := command.CommandContext{
			Buffer:  m.buf,
			Cursors: m.cursors,
		}
		res := m.registry.Execute("edit.newline", ctx)
		if res.Err == nil {
			m = m.applyOperation(res, "edit.newline")
			m = m.syncDisplay()
			m = m.ScrollToCursor()
			return m, tea.Batch(*cmds...)
		}
	}

	// Cancel: Escape key routes to multicursor.escape (no resolver binding)
	if msg.Code == tea.KeyEscape && msg.Mod == 0 {
		ctx := command.CommandContext{
			Buffer:  m.buf,
			Cursors: m.cursors,
		}
		res := m.registry.Execute("multicursor.escape", ctx)
		if res.Err == nil && res.Operation.Kind != command.OperationNone {
			m = m.applyOperation(res, "multicursor.escape")
			m = m.syncDisplay()
			m = m.ScrollToCursor()
			return m, tea.Batch(*cmds...)
		}
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
	newResolver, resResult := m.resolver.Resolve(chord, resCtx)
	m.resolver = newResolver
	switch resResult.Kind {
	case keybind.ResultFound:
		contentHeight := m.contentHeight()
		topRow := m.viewport.TopRow
		scrollCol := m.viewport.ScrollCol
		totalRows := m.snapshot.TotalRows
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
			TotalRows:      func() int { return totalRows },
			ReadOnly:       m.readOnly,
		})
		if res.Cmd != nil {
			*cmds = append(*cmds, res.Cmd)
		}
		m = m.applyOperation(res, resResult.Command)
		m = m.syncDisplay()
		// A scroll operation moves the viewport intentionally; following the
		// cursor would cancel it (critical for read-only docs whose hidden
		// cursor sits at the top).
		if res.Operation.Kind != command.OperationScroll {
			m = m.ScrollToCursor()
		}
	case keybind.ResultMoreChordsNeeded:
		// Chord incomplete — wait for next key
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
			if res.Err == nil && res.Operation.Kind != command.OperationNone {
				m = m.applyOperation(res, "edit.insert-character")
				m = m.syncDisplay()
				m = m.ScrollToCursor()
			}
		}
	}

	return m, tea.Batch(*cmds...)
}
