package textedit

import (
	"regexp"
	"sort"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

var leadingWhitespaceRe = regexp.MustCompile(`^[ \t]*`)

type editInfoItem struct {
	edit buffer.Edit
	cID  int
}

func sortInfosDescending(infos []editInfoItem) {
	sort.Slice(infos, func(i, j int) bool {
		if infos[i].edit.Start == infos[j].edit.Start {
			return infos[i].edit.End > infos[j].edit.End
		}
		return infos[i].edit.Start > infos[j].edit.Start
	})
}

// infosToEdits strips editInfoItem down to the plain []buffer.Edit expected
// by ApplyEdits. Deliberately does NOT copy cID onto Edit.CursorID here: this
// helper is shared by both selection-exact commands (delete/insert/newline,
// where the edit range equals the cursor's own selection — see SEL-EDIT) and
// line-oriented commands (delete-line, clone/move-line, indent/dedent, whose
// edit range is a whole line regardless of any selection). Only the former
// group's call sites set buffer.Edit.CursorID themselves, on the literal, so
// SEL-EDIT's per-cursor attribution isn't force-fed a line-oriented edit that
// was never meant to match the selection bounds.
func infosToEdits(infos []editInfoItem) []buffer.Edit {
	edits := make([]buffer.Edit, len(infos))
	for i, info := range infos {
		edits[i] = info.edit
	}
	return edits
}

// computePostEditCursors derives each cursor's post-edit position from its
// own edit's Insert length — this subsumes what used to be a separate
// "fixed insertLen" variant (insert-char/indent passed a constant), since a
// constant insertLen and len(info.edit.Insert) are the same number whenever
// every edit in the batch inserts the same fixed text.
func computePostEditCursors(infos []editInfoItem) []cursor.Cursor {
	var newCursors []cursor.Cursor
	shift := 0
	for i := len(infos) - 1; i >= 0; i-- {
		info := infos[i]
		insLen := len(info.edit.Insert)
		newPos := info.edit.Start + shift + insLen
		newCursors = append(newCursors, cursor.Cursor{
			Position: newPos,
			Anchor:   newPos,
			ID:       info.cID,
		})
		shift += insLen - (info.edit.End - info.edit.Start)
	}
	return newCursors
}

func buildEditResultFromInfos(infos []editInfoItem) command.Result {
	edits := infosToEdits(infos)
	sorted := buffer.CloneAndSortEditsDescending(edits)
	newCursors := computePostEditCursors(infos)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   sorted,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
	}
}

// noneResult is the standard "nothing to do" command.Result shared by every
// per-cursor/per-line command below.
func noneResult() command.Result {
	return command.Result{Operation: command.Operation{Kind: command.OperationNone}}
}

// perCursorSelectionEdits is the shared driver for selection-exact commands
// (insert-char, newline, delete-left/right, delete-word-left/right, paste):
// one edit per cursor, replacing its selection when it has one or a
// caller-defined "bare" (no-selection) range otherwise. bare returning
// ok=false skips that cursor entirely (e.g. delete-left at buffer start).
//
// This is the SEL-EDIT CursorID-tagging boundary: every edit produced here
// (both the selection and bare branches) carries its originating cursor's ID
// on buffer.Edit.CursorID — not just editInfoItem.cID, which every driver
// uses for post-edit cursor recomputation regardless — so SEL-EDIT can
// attribute the edit range to the exact cursor whose selection it replaced.
// perLineEdits below never tags Edit.CursorID: it is line-oriented, and an
// edit's range there is a whole line, not a cursor's own selection.
func perCursorSelectionEdits(ctx command.CommandContext,
	textFor func(i int, c cursor.Cursor) string,
	bare func(c cursor.Cursor) (start, end int, ok bool)) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return noneResult()
	}

	var infos []editInfoItem
	for i, c := range all {
		var e buffer.Edit
		if c.HasSelection() {
			e = buffer.Edit{Start: c.SelectionStart(), End: selectionEndInclusive(c, ctx.Buffer), Insert: textFor(i, c), CursorID: c.ID}
		} else {
			start, end, ok := bare(c)
			if !ok {
				continue
			}
			e = buffer.Edit{Start: start, End: end, Insert: textFor(i, c), CursorID: c.ID}
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return noneResult()
	}
	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos)
}

// perLineEdits is the shared driver for line-oriented commands (delete-line,
// clone-line-up/down, indent, outdent): one edit per (deduped) target line.
// dedupe=true skips a line already visited by an earlier cursor in this same
// batch (indent/dedent/delete-line must not double-edit a line two cursors
// share); dedupe=false lets every cursor produce its own edit even when they
// share a line (clone-line-up/down, where each clone is independent). build
// returning ok=false skips that cursor/line (e.g. clone-up at line 0).
func perLineEdits(ctx command.CommandContext, dedupe bool,
	build func(line int, c cursor.Cursor) (buffer.Edit, bool)) command.Result {
	all := ctx.Cursors.All()
	if len(all) == 0 {
		return noneResult()
	}

	var infos []editInfoItem
	seen := map[int]bool{}
	for _, c := range all {
		bp := ctx.Buffer.OffsetToLineCol(c.Position)
		if dedupe {
			if seen[bp.Line] {
				continue
			}
			seen[bp.Line] = true
		}
		e, ok := build(bp.Line, c)
		if !ok {
			continue
		}
		infos = append(infos, editInfoItem{edit: e, cID: c.ID})
	}

	if len(infos) == 0 {
		return noneResult()
	}
	sortInfosDescending(infos)
	return buildEditResultFromInfos(infos)
}
