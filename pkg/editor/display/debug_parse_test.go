package display_test

import (
	"testing"
	"fmt"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

func TestDebugTaskList(t *testing.T) {
	texts := []string{
		"- [ ] todo item",
		"other\n\n- [ ] todo item",
		"- [x] done",
	}
	sMap := display.NewSyntaxMap()
	for _, text := range texts {
		buf := buffer.New(text)
		_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
		fmt.Printf("=== %q (%d lines) ===\n", text, buf.LineCount())
		for lineIdx, line := range snap.Lines {
			for _, sp := range line.Spans {
				fmt.Printf("  L%d: kind=%d state=%d text=%q bufStart=%d bufEnd=%d\n",
					lineIdx, sp.Kind, sp.State, sp.Text, sp.BufferStart, sp.BufferEnd)
			}
		}
	}
}

func TestDebugHR(t *testing.T) {
	texts := []string{
		"---",
		"other\n\n---",
		"***",
		"___",
	}
	sMap := display.NewSyntaxMap()
	for _, text := range texts {
		buf := buffer.New(text)
		_, snap := sMap.Sync(buf, cursor.NewCursorSet(0))
		fmt.Printf("=== %q (%d lines) ===\n", text, buf.LineCount())
		for lineIdx, line := range snap.Lines {
			for _, sp := range line.Spans {
				fmt.Printf("  L%d: kind=%d state=%d text=%q bufStart=%d bufEnd=%d\n",
					lineIdx, sp.Kind, sp.State, sp.Text, sp.BufferStart, sp.BufferEnd)
			}
		}
	}
}
