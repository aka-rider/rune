package styles

import "charm.land/lipgloss/v2"

type Styles struct {
	ActiveBorder   lipgloss.Style
	InactiveBorder lipgloss.Style

	PaneTitle    lipgloss.Style
	FileNormal   lipgloss.Style
	FileSelected lipgloss.Style
	DirSuffix    lipgloss.Style

	TabsDivider lipgloss.Style
	TabNormal   lipgloss.Style
	TabActive   lipgloss.Style
	TabPinned   lipgloss.Style
	TabDirty    lipgloss.Style

	Breadcrumb    lipgloss.Style
	BreadcrumbSep lipgloss.Style

	Footer     lipgloss.Style
	FooterKey  lipgloss.Style
	FooterHint lipgloss.Style
	FooterMeta lipgloss.Style

	Error lipgloss.Style

	CodeBlockBg    lipgloss.Style
	CodeBlockLabel lipgloss.Style
	CodeKeyword    lipgloss.Style
	CodeString     lipgloss.Style
	CodeComment    lipgloss.Style
	CodeFunction   lipgloss.Style
	CodeType       lipgloss.Style
	CodeNumber     lipgloss.Style
	CodeOperator   lipgloss.Style
	CodePlain      lipgloss.Style

	HeadingH1  lipgloss.Style
	Heading    lipgloss.Style
	HeadingH6  lipgloss.Style
	InlineCode lipgloss.Style

	TaskChecked   lipgloss.Style
	TaskUnchecked lipgloss.Style

	MdBold          lipgloss.Style
	MdItalic        lipgloss.Style
	MdStrikethrough lipgloss.Style

	TableHeader    lipgloss.Style
	TableSeparator lipgloss.Style

	Selection lipgloss.Style

	ChatTitle        lipgloss.Style
	ChatUserMsg      lipgloss.Style
	ChatAssistantMsg lipgloss.Style
	ChatDivider      lipgloss.Style
	ChatLoading      lipgloss.Style
	ChatInput        lipgloss.Style
}

func Default() Styles {
	subtle := lipgloss.Color("241")
	highlight := lipgloss.Color("212")
	special := lipgloss.Color("153")
	errColor := lipgloss.Color("196")

	border := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(subtle)

	return Styles{
		ActiveBorder:   border.BorderForeground(highlight),
		InactiveBorder: border,

		PaneTitle:    lipgloss.NewStyle().Bold(true).Foreground(special).Padding(0, 1),
		FileNormal:   lipgloss.NewStyle().Padding(0, 1),
		FileSelected: lipgloss.NewStyle().Padding(0, 1).Foreground(highlight).Bold(true),
		DirSuffix:    lipgloss.NewStyle().Foreground(subtle),

		TabsDivider: lipgloss.NewStyle().Foreground(subtle),
		TabNormal:   lipgloss.NewStyle().Padding(0, 1).Foreground(subtle),
		TabActive:   lipgloss.NewStyle().Padding(0, 1).Foreground(highlight).Bold(true),
		TabPinned:   lipgloss.NewStyle().Foreground(special),
		TabDirty:    lipgloss.NewStyle().Foreground(errColor),

		Breadcrumb:    lipgloss.NewStyle().Foreground(special).Padding(0, 1),
		BreadcrumbSep: lipgloss.NewStyle().Foreground(subtle),

		Footer:     lipgloss.NewStyle().Background(lipgloss.Color("236")).Padding(0, 1),
		FooterKey:  lipgloss.NewStyle().Foreground(highlight).Background(lipgloss.Color("236")).Bold(true),
		FooterHint: lipgloss.NewStyle().Foreground(subtle).Background(lipgloss.Color("236")),
		FooterMeta: lipgloss.NewStyle().Foreground(special).Background(lipgloss.Color("236")),

		Error: lipgloss.NewStyle().Foreground(errColor).Bold(true),

		CodeBlockBg:    lipgloss.NewStyle().Background(lipgloss.Color("235")),
		CodeBlockLabel: lipgloss.NewStyle().Foreground(subtle).Italic(true),
		CodeKeyword:    lipgloss.NewStyle().Foreground(lipgloss.Color("204")).Background(lipgloss.Color("235")),
		CodeString:     lipgloss.NewStyle().Foreground(lipgloss.Color("114")).Background(lipgloss.Color("235")),
		CodeComment:    lipgloss.NewStyle().Foreground(lipgloss.Color("242")).Background(lipgloss.Color("235")).Italic(true),
		CodeFunction:   lipgloss.NewStyle().Foreground(lipgloss.Color("39")).Background(lipgloss.Color("235")),
		CodeType:       lipgloss.NewStyle().Foreground(lipgloss.Color("179")).Background(lipgloss.Color("235")),
		CodeNumber:     lipgloss.NewStyle().Foreground(lipgloss.Color("180")).Background(lipgloss.Color("235")),
		CodeOperator:   lipgloss.NewStyle().Foreground(lipgloss.Color("249")).Background(lipgloss.Color("235")),
		CodePlain:      lipgloss.NewStyle().Foreground(lipgloss.Color("252")).Background(lipgloss.Color("235")),

		HeadingH1:  lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("228")).Background(lipgloss.Color("63")),
		Heading:    lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("39")),
		HeadingH6:  lipgloss.NewStyle().Foreground(lipgloss.Color("35")),
		InlineCode: lipgloss.NewStyle().Foreground(lipgloss.Color("203")).Background(lipgloss.Color("236")),

		TaskChecked:   lipgloss.NewStyle().Foreground(lipgloss.Color("35")),
		TaskUnchecked: lipgloss.NewStyle().Foreground(lipgloss.Color("240")),

		MdBold:          lipgloss.NewStyle().Bold(true),
		MdItalic:        lipgloss.NewStyle().Italic(true),
		MdStrikethrough: lipgloss.NewStyle().Strikethrough(true),

		TableHeader:    lipgloss.NewStyle().Bold(true),
		TableSeparator: lipgloss.NewStyle().Foreground(lipgloss.Color("240")),

		Selection: lipgloss.NewStyle().Background(lipgloss.Color("239")),

		ChatTitle:        lipgloss.NewStyle().Bold(true).Foreground(special).Padding(0, 1),
		ChatUserMsg:      lipgloss.NewStyle().Foreground(subtle),
		ChatAssistantMsg: lipgloss.NewStyle().Foreground(lipgloss.Color("252")),
		ChatDivider:      lipgloss.NewStyle().Foreground(subtle),
		ChatLoading:      lipgloss.NewStyle().Foreground(subtle).Italic(true),
		ChatInput:        lipgloss.NewStyle().Foreground(highlight),
	}
}
