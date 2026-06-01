package editor

import (
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/styles"
)

func classToStyle(class string, st styles.Styles) lipgloss.Style {
	switch class {
	case "keyword":
		return st.CodeKeyword
	case "string":
		return st.CodeString
	case "comment":
		return st.CodeComment
	case "function":
		return st.CodeFunction
	case "type":
		return st.CodeType
	case "number":
		return st.CodeNumber
	case "operator":
		return st.CodeOperator
	default:
		return st.CodePlain
	}
}

type selInterval struct{ start, end int }

// isInSelection reports whether the byte offset falls within any selection interval.
func isInSelection(off int, selections []selInterval) bool {
	for _, sel := range selections {
		if off >= sel.start && off < sel.end {
			return true
		}
	}
	return false
}
