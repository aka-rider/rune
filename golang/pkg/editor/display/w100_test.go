package display_test

import (
	"fmt"
	"strings"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

func TestTable_ThreeColWrapped(t *testing.T) {
	md := `| Concern | Choice | Rationale |
|---|---|---|
| Language | Python 3.12+ | team skills; modern async |
| API / app | Django + Django Ninja + Channels | matches the team's existing Django backend; Ninja gives async, Pydantic-typed endpoints + OpenAPI |
| Browser | Playwright (async API) | auto-wait, resilient locators, native network capture |
| DB | PostgreSQL (existing Aurora) | keep infra; Django migrations replace Liquibase |`

	buf := buffer.New(md)
	width := 80
	sMap := display.NewSyntaxMap().SetWidth(width)
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))
	wm := display.NewWrapMap(width)
	ws := wm.Sync(snap)
	ds := display.BuildSnapshot(ws)
	ds = display.ExpandTableRows(ds)

	for i, line := range ds.Lines {
		text := ""
		for _, sp := range line.Spans {
			text += sp.Text
		}
		w := 0
		for range text {
			w++
		}
		if strings.ContainsRune(text, '┌') || strings.ContainsRune(text, '└') || i < 25 {
			fmt.Printf("Line %2d (w=%3d): %s\n", i, w, text)
		}
	}
}
