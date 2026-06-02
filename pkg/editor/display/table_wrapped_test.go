package display_test

import (
	"fmt"
	"strings"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

func TestTable_WrappedLayoutWordWraps(t *testing.T) {
	// 3-column table that needs wrapping at width=80
	md := `| Concern | Choice | Rationale |
|---|---|---|
| Language | Python 3.12+ | team skills; modern async |
| API / app | Django + Django Ninja + Channels | matches the team's existing Django backend; Ninja gives async |
| Browser | Playwright (async API) | auto-wait, resilient locators, native network capture |`

	buf := buffer.New(md)
	width := 80
	sMap := display.NewSyntaxMap().SetWidth(width)
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))
	wm := display.NewWrapMap(width)
	ws := wm.Sync(snap)
	ds := display.BuildSnapshot(ws)
	ds = display.ExpandTableRows(ds)

	// Should have more display rows than source lines due to wrapping
	sourceLines := 5 // header + sep + 3 body rows
	if len(ds.Lines) <= sourceLines {
		t.Errorf("Expected wrapped rows to produce more display lines than source (%d), got %d",
			sourceLines, len(ds.Lines))
	}

	// Every non-empty display line should have │ borders
	for i, line := range ds.Lines {
		text := ""
		for _, sp := range line.Spans {
			text += sp.Text
		}
		if text == "" {
			continue
		}
		// Allow top/bottom/body separators
		if strings.ContainsRune(text, '┌') || strings.ContainsRune(text, '└') || strings.ContainsRune(text, '├') {
			continue
		}
		if !strings.ContainsRune(text, '│') {
			t.Errorf("Line %d missing │ borders: %q", i, text)
		}
	}

	// Print for visual verification
	for i, line := range ds.Lines {
		text := ""
		for _, sp := range line.Spans {
			text += sp.Text
		}
		fmt.Printf("Line %2d: %s\n", i, text)
	}
}

func TestTable_WrappedLayoutURLNotBroken(t *testing.T) {
	// A table where the URL fits when given the full column width
	md := `| Name | Link |
|---|---|
| Test | http://localhost:7900/?autoconnect=1&resize=scale&password=secret |`

	buf := buffer.New(md)
	// At width 100, URL (65 chars) fits: budget=93, floor=[4,65]=69, remaining=24
	width := 100
	sMap := display.NewSyntaxMap().SetWidth(width)
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))
	wm := display.NewWrapMap(width)
	ws := wm.Sync(snap)
	ds := display.BuildSnapshot(ws)
	ds = display.ExpandTableRows(ds)

	url := "http://localhost:7900/?autoconnect=1&resize=scale&password=secret"

	found := false
	for _, line := range ds.Lines {
		text := ""
		for _, sp := range line.Spans {
			text += sp.Text
		}
		if strings.Contains(text, url) {
			found = true
			break
		}
	}
	if !found {
		t.Errorf("URL should appear intact (not broken across lines)")
		for i, line := range ds.Lines {
			text := ""
			for _, sp := range line.Spans {
				text += sp.Text
			}
			fmt.Printf("Line %2d: %s\n", i, text)
		}
	}
}

func TestTable_PivotOnlyWhenVeryNarrow(t *testing.T) {
	// Table with 65-char URL: at width 80, budget=73, floor=[7,65]=72, flex=1
	// UI column gets ~8 chars which is too narrow → should pivot
	md := `| UI | URL |
|---|---|
| Swagger (API explorer) | http://localhost:8080/swagger-ui.html |
| Selenium live browser view (VNC) | http://localhost:7900/?autoconnect=1&resize=scale&password=secret |`

	buf := buffer.New(md)
	sMap := display.NewSyntaxMap().SetWidth(80)
	_, snap := sMap.SyncNoReveal(buf, cursor.NewCursorSet(0))

	foundPivot := false
	for _, line := range snap.Lines {
		for _, sp := range line.Spans {
			if sp.TableLayout == display.TableLayoutPivoted {
				foundPivot = true
			}
		}
	}
	if !foundPivot {
		t.Fatal("Expected Pivoted layout for 2-col table with 65-char URL at width=80")
	}
}
