package image

import (
	"errors"
	"fmt"
	"strings"
	"sync"

	tea "charm.land/bubbletea/v2"
	uv "github.com/charmbracelet/ultraviolet"

	"rune/pkg/imagekit"
)

var writeTTYMu sync.Mutex

func writeTTY(seq string) error {
	if seq == "" {
		return nil
	}
	writeTTYMu.Lock()
	defer writeTTYMu.Unlock()

	inTty, outTty, err := uv.OpenTTY()
	if err != nil {
		return fmt.Errorf("open tty: %w", err)
	}
	defer func() {
		if inTty != nil {
			_ = inTty.Close()
		}
		if outTty != nil {
			_ = outTty.Close()
		}
	}()
	_, err = outTty.WriteString(seq)
	if err != nil {
		return err
	}
	return nil
}

func DecodeCmd(m Model) tea.Cmd {
	p, ap, mt, mc, mr, c, fsys := m.path, m.absPath, m.mtime, m.maxCols, m.maxRows, m.cellSize, m.fs
	return func() tea.Msg {
		data, err := fsys.ReadFile(ap)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}

		if imagekit.SniffFormat(data) == "gif" {
			if gif, aerr := imagekit.DecodeGIF(data); aerr == nil {
				cols, rows := imagekit.FitCells(gif.Width, gif.Height, mc, mr, c)
				return UpdateMsg{Path: p, inner: decodedMsg{
					path: p, cols: cols, rows: rows,
					pxW: gif.Width, pxH: gif.Height, mtime: mt,
					animated: true, frameCount: len(gif.Frames),
					delays: gif.Delays, loopCount: gif.LoopCount,
				}}
			} else if !errors.Is(aerr, imagekit.ErrNotAnimated) {
				return ErrorMsg{Path: p, Err: aerr}
			}
		}

		dec, err := imagekit.DecodeStill(data)
		if err != nil {
			return ErrorMsg{Path: p, Err: err}
		}
		cols, rows := imagekit.FitCells(dec.Width, dec.Height, mc, mr, c)
		return UpdateMsg{Path: p, inner: decodedMsg{
			path: p, cols: cols, rows: rows, pxW: dec.Width, pxH: dec.Height, mtime: mt,
		}}
	}
}

func TransmitCmd(m Model) tea.Cmd {
	p, ap, theID, c, r, sz, fsys := m.path, m.absPath, m.id, m.cols, m.rows, m.cellSize, m.fs
	return func() tea.Msg {
		data, err := fsys.ReadFile(ap)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}
		dec, err := imagekit.DecodeStill(data)
		if err != nil {
			return ErrorMsg{Path: p, Err: err}
		}
		tw, th := imagekit.FitBox(dec.Width, dec.Height, c*sz.W, r*sz.H)
		resized := imagekit.Resize(dec.Image, tw, th)
		seq, err := imagekit.EncodeTransmit(resized, theID, c, r)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("encode image %q: %w", ap, err)}
		}
		if err := writeTTY(seq); err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("transmit image %q: %w", ap, err)}
		}
		return UpdateMsg{Path: p, inner: transmittedMsg{path: p}}
	}
}

func TransmitAnimationCmd(m Model) tea.Cmd {
	p, ap, ids, c, r, sz, fsys := m.path, m.absPath, append([]uint32(nil), m.anim.frameIDs...), m.cols, m.rows, m.cellSize, m.fs
	return func() tea.Msg {
		data, err := fsys.ReadFile(ap)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}
		gif, err := imagekit.DecodeGIF(data)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("decode gif %q: %w", ap, err)}
		}
		tw, th := imagekit.FitBox(gif.Width, gif.Height, c*sz.W, r*sz.H)
		var seq strings.Builder
		for i, frame := range gif.Frames {
			if i >= len(ids) {
				break
			}
			resized := imagekit.Resize(frame, tw, th)
			s, encErr := imagekit.EncodeTransmit(resized, ids[i], c, r)
			if encErr != nil {
				return ErrorMsg{Path: p, Err: fmt.Errorf("encode frame %d of %q: %w", i, ap, encErr)}
			}
			seq.WriteString(s)
		}
		if err := writeTTY(seq.String()); err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("transmit animation %q: %w", ap, err)}
		}
		return UpdateMsg{Path: p, inner: transmittedMsg{path: p}}
	}
}

func EncodeITerm2Cmd(m Model) tea.Cmd {
	p, ap, c, r, sz, fsys := m.path, m.absPath, m.cols, m.rows, m.cellSize, m.fs
	return func() tea.Msg {
		data, err := fsys.ReadFile(ap)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}
		dec, err := imagekit.DecodeStill(data)
		if err != nil {
			return ErrorMsg{Path: p, Err: err}
		}
		tw, th := imagekit.FitBox(dec.Width, dec.Height, c*sz.W, r*sz.H)
		resized := imagekit.Resize(dec.Image, tw, th)
		slices, err := imagekit.EncodeITerm2Rows(resized, c, r, sz)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("encode iterm2 %q: %w", ap, err)}
		}
		return UpdateMsg{Path: p, inner: encodedMsg{path: p, slices: slices}}
	}
}

// DeleteAllCmd deletes every image the terminal is currently displaying.
// Fire-and-forget.
func DeleteAllCmd() tea.Cmd {
	return func() tea.Msg {
		_ = writeTTY(imagekit.EncodeDeleteAll())
		return nil
	}
}

// RetransmitCmd initiates transmission based on current sizes (e.g. after layout changes)
func (m Model) RetransmitCmd() tea.Cmd {
	if m.state != PendingTransmit && m.state != Live {
		return nil
	}
	if m.cols <= 0 || m.rows <= 0 {
		return nil
	}
	if m.termCaps.SupportsKittyGraphics() {
		if m.anim.animated && len(m.anim.frameIDs) > 0 {
			return TransmitAnimationCmd(m)
		}
		return TransmitCmd(m)
	}
	return EncodeITerm2Cmd(m)
}
