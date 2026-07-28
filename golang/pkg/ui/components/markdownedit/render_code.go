package markdownedit

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
