package editor

import (
	"time"

	tea "charm.land/bubbletea/v2"
)

// imageFrameTickMsg fires when an animated image should advance to frame `next`.
// `gen` guards against stale ticks from a superseded animation chain.
type imageFrameTickMsg struct {
	path string
	gen  int
	next int
}

// scheduleImageFrame returns a Cmd that, after d, asks to advance the image at
// path to frame `next`. Locals are captured before the closure (§5.5).
func (m Model) scheduleImageFrame(path string, gen, next int, d time.Duration) tea.Cmd {
	p, g, n, dur := path, gen, next, d
	return tea.Tick(dur, func(time.Time) tea.Msg {
		return imageFrameTickMsg{path: p, gen: g, next: n}
	})
}

// armImageTicks starts a frame-tick chain for every visible, live, animated
// image that is not already ticking and has not finished looping. Off-screen
// images are not ticked (the chain resumes when they scroll back into view).
func (m Model) armImageTicks() (Model, tea.Cmd) {
	if !m.imageKittyCapable() {
		return m, nil
	}
	var cmds []tea.Cmd
	for path := range m.images.byPath {
		e, ok := m.images.get(path)
		if !ok || !e.animated || e.state != live || e.frameCount <= 1 {
			continue
		}
		if e.ticking || animationShouldStop(e) || !m.imageVisible(path) {
			continue
		}
		e.ticking = true
		e.tickGen++
		m.images = m.images.upsert(e)
		cmds = append(cmds, m.scheduleImageFrame(path, e.tickGen, e.frameIdx+1, frameDelay(e)))
	}
	if len(cmds) == 0 {
		return m, nil
	}
	return m, tea.Batch(cmds...)
}

// handleImageFrameTick advances an animated image one frame (changing only the
// placeholder fg color, never the geometry) and schedules the next tick while
// the image remains visible and the loop budget allows.
func (m Model) handleImageFrameTick(msg imageFrameTickMsg) (Model, tea.Cmd) {
	e, ok := m.images.get(msg.path)
	if !ok || !e.animated || e.state != live || msg.gen != e.tickGen {
		return m, nil // stale tick or image gone
	}

	next := msg.next
	if next >= e.frameCount { // completed a full loop
		e.loopsDone++
		next = 0
		if animationShouldStop(e) {
			e.frameIdx = e.frameCount - 1 // rest on the final frame
			e.ticking = false
			m.images = m.images.upsert(e)
			return m, nil
		}
	}
	e.frameIdx = next

	if !m.imageVisible(msg.path) {
		e.ticking = false // pause; armImageTicks resumes on scroll-in
		m.images = m.images.upsert(e)
		return m, nil
	}

	m.images = m.images.upsert(e)
	return m, m.scheduleImageFrame(e.path, e.tickGen, e.frameIdx+1, frameDelay(e))
}

// animationShouldStop reports whether an animated image has exhausted its loop
// budget. LoopCount 0 means loop forever; <0 means play once.
func animationShouldStop(e imageEntry) bool {
	switch {
	case e.loopCount == 0:
		return false
	case e.loopCount < 0:
		return e.loopsDone >= 1
	default:
		return e.loopsDone >= e.loopCount
	}
}

// frameDelay returns the delay to wait while the current frame is shown.
func frameDelay(e imageEntry) time.Duration {
	if e.frameIdx >= 0 && e.frameIdx < len(e.delays) {
		return e.delays[e.frameIdx]
	}
	return 100 * time.Millisecond
}

// imageVisible reports whether any reserved row of the image at path lies within
// the current viewport slice.
func (m Model) imageVisible(path string) bool {
	top := m.viewport.TopRow
	bottom := top + m.contentHeight()
	for r, l := range m.snapshot.Lines {
		if l.ImagePath == path && r >= top && r < bottom {
			return true
		}
	}
	return false
}
