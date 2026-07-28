package editortest

import tea "charm.land/bubbletea/v2"

// TypeText delivers s to the model one keystroke per rune, as real terminal
// input would arrive (both Code and Text set, matching bubbletea v2's key
// decoding for printable runes). Cmds returned by each keystroke are
// discarded — callers that need the async round trip settled use Drain on
// the final state, or their package's settle helper per keystroke.
func TypeText[M any](m M, update func(M, tea.Msg) (M, tea.Cmd), s string) M {
	for _, r := range s {
		m, _ = update(m, tea.KeyPressMsg{Code: r, Text: string(r)})
	}
	return m
}
