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
}

func Default() Styles {
	subtle    := lipgloss.Color("241")
	highlight := lipgloss.Color("212")
	special   := lipgloss.Color("153")
	errColor  := lipgloss.Color("196")

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
	}
}
