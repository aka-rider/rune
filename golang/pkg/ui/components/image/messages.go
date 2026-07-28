package image

import (
	"time"

	tea "charm.land/bubbletea/v2"
)

// UpdateMsg is an envelope that routes internal image lifecycle messages
// to the correct image model without exporting the internal message types.
// Gen is the spawning instance's spawn generation (Model.gen) — Update drops
// the message if it no longer matches, so a same-path result from a
// despawned/respawned instance (mtime replacement) can never land on the
// wrong instance even though Path alone would match.
type UpdateMsg struct {
	Path  string
	Gen   uint64
	inner tea.Msg
}

// ReadyMsg is emitted when an image has been successfully decoded (and
// transmitted/encoded if applicable) and is ready for display.
type ReadyMsg struct {
	Path string
}

// ErrorMsg is emitted when an image fails to decode, transmit, or encode.
// Gen guards it exactly like UpdateMsg (see its doc comment).
type ErrorMsg struct {
	Path string
	Gen  uint64
	Err  error
}

// Internal lifecycle messages.

type decodedMsg struct {
	path       string
	cols, rows int
	pxW, pxH   int
	mtime      int64
	animated   bool
	frameCount int
	delays     []time.Duration
	loopCount  int
}

type transmittedMsg struct {
	path string
}

type encodedMsg struct {
	path   string
	slices []string
}

// frameTickMsg fires when an animated image should advance to the next frame.
// `gen` guards against stale ticks from a superseded animation chain.
type frameTickMsg struct {
	path string
	gen  int
	next int
}
