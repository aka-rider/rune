package command

import (
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
)

type ArgSpec struct {
	Name     string
	Type     string // "string", "int", "bool"
	Required bool
}

type CommandContext struct {
	Buffer       buffer.Buffer
	Cursors      cursor.CursorSet
	FilePath     string
	Args         map[string]any
	Now          time.Time
	NewRequestID func() string
	HashContent  func(string) string
	Selection    func() string
	LineCount    func() int
	ReadOnly     bool

	// Navigation capabilities
	BufferToSyntax func(coords.BufferPoint) coords.SyntaxPoint
	SyntaxToBuffer func(coords.SyntaxPoint) coords.BufferPoint
	SyntaxToWrap   func(coords.SyntaxPoint) coords.WrapPoint
	WrapToSyntax   func(coords.WrapPoint) coords.SyntaxPoint
	WrapVisualCol  func(row, byteCol int) int
	WrapByteCol    func(row, visualCol int) int
	ViewportBounds func() (topRow, bottomRow int)
	ScrollCol      func() int
	ViewportHeight func() int
	SoftWrap       func() bool
	WrapRowCount   func() int
}

type OperationKind int

const (
	OperationNone OperationKind = iota
	OperationMoveCursors
	OperationEditBuffer
	OperationScroll
	OperationHistory
	OperationSaveFile
)

type Operation struct {
	Kind            OperationKind
	Edits           []buffer.Edit
	Cursors         cursor.CursorSet
	ScrollDY        int
	ScrollDX        int
	SavePath        string
	SaveContent     string
	SaveRequestID   string
	SaveContentHash string
}

type Result struct {
	Operation Operation
	Cmd       tea.Cmd
	Err       error
}

type CommandFn func(ctx CommandContext) Result

type Command struct {
	Name     string
	Category string
	Title    string
	Execute  CommandFn
	Args     []ArgSpec
	When     string
}
