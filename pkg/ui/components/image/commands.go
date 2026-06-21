package image

import (
	"errors"
	"fmt"
	"os"
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
	p, ap, mt, mc, mr, c := m.path, m.absPath, m.mtime, m.maxCols, m.maxRows, m.cellSize
	return func() tea.Msg {
		data, err := os.ReadFile(ap)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}

		if imagekit.SniffFormat(data) == "gif" {
			if anim, aerr := imagekit.DecodeGIF(data); aerr == nil {
				cols, rows := imagekit.FitCells(anim.Width, anim.Height, mc, mr, c)
				return UpdateMsg{Path: p, inner: decodedMsg{
					path: p, cols: cols, rows: rows,
					pxW: anim.Width, pxH: anim.Height, mtime: mt,
					animated: true, frameCount: len(anim.Frames),
					delays: anim.Delays, loopCount: anim.LoopCount,
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
	p, ap, theID, c, r, sz := m.path, m.absPath, m.id, m.cols, m.rows, m.cellSize
	return func() tea.Msg {
		data, err := os.ReadFile(ap)
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
	p, ap, ids, c, r, sz := m.path, m.absPath, append([]uint32(nil), m.frameIDs...), m.cols, m.rows, m.cellSize
	return func() tea.Msg {
		data, err := os.ReadFile(ap)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}
		anim, err := imagekit.DecodeGIF(data)
		if err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("decode gif %q: %w", ap, err)}
		}
		tw, th := imagekit.FitBox(anim.Width, anim.Height, c*sz.W, r*sz.H)
		var seq string
		for i, frame := range anim.Frames {
			if i >= len(ids) {
				break
			}
			resized := imagekit.Resize(frame, tw, th)
			s, encErr := imagekit.EncodeTransmit(resized, ids[i], c, r)
			if encErr != nil {
				return ErrorMsg{Path: p, Err: fmt.Errorf("encode frame %d of %q: %w", i, ap, encErr)}
			}
			seq += s
		}
		if err := writeTTY(seq); err != nil {
			return ErrorMsg{Path: p, Err: fmt.Errorf("transmit animation %q: %w", ap, err)}
		}
		return UpdateMsg{Path: p, inner: transmittedMsg{path: p}}
	}
}

func EncodeITerm2Cmd(m Model) tea.Cmd {
	p, ap, c, r, sz := m.path, m.absPath, m.cols, m.rows, m.cellSize
	return func() tea.Msg {
		data, err := os.ReadFile(ap)
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

// DeleteCmd deletes the given image IDs from the terminal. Fire-and-forget.
func DeleteAllCmd() tea.Cmd {
	return func() tea.Msg {
		_ = writeTTY(imagekit.EncodeDeleteAll())
		return nil
	}
}

// DeleteCmd deletes the given image IDs from the terminal. Fire-and-forget.
func DeleteCmd(ids []uint32) tea.Cmd {
	captured := append([]uint32(nil), ids...)
	return func() tea.Msg {
		if len(captured) == 0 {
			return nil
		}
		var seq string
		for _, id := range captured {
			seq += imagekit.EncodeDelete(id)
		}
		_ = writeTTY(seq)
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
		if m.animated && len(m.frameIDs) > 0 {
			return TransmitAnimationCmd(m)
		}
		return TransmitCmd(m)
	}
	return EncodeITerm2Cmd(m)
}
