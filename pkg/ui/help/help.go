// Package help renders the in-app help document: a read-only markdown page
// whose keybinding table is generated from the keymap, so it can never drift
// from the actual bindings. The surrounding prose is authored in help.md and
// embedded at build time.
package help

import (
	_ "embed"
	"strings"

	"rune/pkg/ui/keymap"
)

// DocPath is the sentinel identity of the help document. It is used as the
// "path" of the help tab; it never touches the filesystem (every load path
// short-circuits before disk I/O) and cannot collide with a real file path or
// the "" untitled buffer.
const DocPath = "rune://help"

// keybindingsPlaceholder marks where the generated shortcut table is spliced
// into the embedded template.
const keybindingsPlaceholder = "<!-- KEYBINDINGS -->"

//go:embed help.md
var templateMD string

// Document renders the help markdown: the embedded prose with a keybindings
// table spliced in at the placeholder. The table is built from keys.AllHelp(),
// the single source of truth for shortcuts.
func Document(keys keymap.Bindings) string {
	var b strings.Builder
	b.WriteString("| Key | Action |\n")
	b.WriteString("| --- | --- |\n")
	for _, e := range keys.AllHelp() {
		b.WriteString("| ")
		b.WriteString(e.Key)
		b.WriteString(" | ")
		b.WriteString(e.Desc)
		b.WriteString(" |\n")
	}
	return strings.Replace(templateMD, keybindingsPlaceholder, b.String(), 1)
}
