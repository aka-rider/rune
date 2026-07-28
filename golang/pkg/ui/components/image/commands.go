package image

import (
	"errors"
	"fmt"
	"strings"
	"sync"

	tea "charm.land/bubbletea/v2"
	uv "github.com/charmbracelet/ultraviolet"

	"rune/pkg/imagekit"
	"rune/pkg/vfs"
)

var writeTTYMu sync.Mutex

// ttyWritesDisabled short-circuits writeTTY to a successful no-op for the
// remainder of the process (DisableTTYWritesForTesting, image_testing.go):
// a test executing a real Transmit*Cmd must neither dump escape bytes into
// the developer's terminal nor fail spuriously when the test runner has no
// controlling TTY.
var ttyWritesDisabled bool

func writeTTY(seq string) error {
	if seq == "" {
		return nil
	}
	writeTTYMu.Lock()
	defer writeTTYMu.Unlock()
	if ttyWritesDisabled {
		return nil
	}

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

// readImageFile reads absPath via fsys, wrapping a read failure into an
// ErrorMsg — the prologue every command function needs before its own
// format-specific decode. errMsg is nil on success.
func readImageFile(fsys vfs.FS, p, ap string, gen uint64) (data []byte, errMsg tea.Msg) {
	data, err := fsys.ReadFile(ap)
	if err != nil {
		return nil, ErrorMsg{Path: p, Gen: gen, Err: fmt.Errorf("read image %q: %w", ap, err)}
	}
	return data, nil
}

// readStill reads absPath and decodes it as a still image, collapsing both
// failure modes (read, decode) into one ErrorMsg return — the read+decode
// pair TransmitCmd and EncodeITerm2Cmd share verbatim. DecodeCmd's own
// still-decode fallback reuses bytes it already read for the GIF sniff, so
// it calls imagekit.DecodeStill directly instead of re-reading here.
func readStill(fsys vfs.FS, p, ap string, gen uint64) (dec imagekit.Decoded, errMsg tea.Msg) {
	data, errMsg := readImageFile(fsys, p, ap, gen)
	if errMsg != nil {
		return imagekit.Decoded{}, errMsg
	}
	dec, err := imagekit.DecodeStill(data)
	if err != nil {
		return imagekit.Decoded{}, ErrorMsg{Path: p, Gen: gen, Err: err}
	}
	return dec, nil
}

func DecodeCmd(m Model) tea.Cmd {
	p, ap, mt, mc, mr, c, fsys, gen := m.path, m.absPath, m.mtime, m.maxCols, m.maxRows, m.cellSize, m.fs, m.gen
	return func() tea.Msg {
		data, errMsg := readImageFile(fsys, p, ap, gen)
		if errMsg != nil {
			return errMsg
		}

		if imagekit.SniffFormat(data) == "gif" {
			if gif, aerr := imagekit.DecodeGIF(data); aerr == nil {
				cols, rows := imagekit.FitCells(gif.Width, gif.Height, mc, mr, c)
				return UpdateMsg{Path: p, Gen: gen, inner: decodedMsg{
					path: p, cols: cols, rows: rows,
					pxW: gif.Width, pxH: gif.Height, mtime: mt,
					animated: true, frameCount: len(gif.Frames),
					delays: gif.Delays, loopCount: gif.LoopCount,
				}}
			} else if !errors.Is(aerr, imagekit.ErrNotAnimated) {
				return ErrorMsg{Path: p, Gen: gen, Err: aerr}
			}
		}

		dec, err := imagekit.DecodeStill(data)
		if err != nil {
			return ErrorMsg{Path: p, Gen: gen, Err: err}
		}
		cols, rows := imagekit.FitCells(dec.Width, dec.Height, mc, mr, c)
		return UpdateMsg{Path: p, Gen: gen, inner: decodedMsg{
			path: p, cols: cols, rows: rows, pxW: dec.Width, pxH: dec.Height, mtime: mt,
		}}
	}
}

func TransmitCmd(m Model) tea.Cmd {
	p, ap, theID, c, r, sz, fsys, gen := m.path, m.absPath, m.id, m.cols, m.rows, m.cellSize, m.fs, m.gen
	return func() tea.Msg {
		dec, errMsg := readStill(fsys, p, ap, gen)
		if errMsg != nil {
			return errMsg
		}
		tw, th := imagekit.FitBox(dec.Width, dec.Height, c*sz.W, r*sz.H)
		resized := imagekit.Resize(dec.Image, tw, th)
		seq, err := imagekit.EncodeTransmit(resized, theID, c, r)
		if err != nil {
			return ErrorMsg{Path: p, Gen: gen, Err: fmt.Errorf("encode image %q: %w", ap, err)}
		}
		if err := writeTTY(seq); err != nil {
			return ErrorMsg{Path: p, Gen: gen, Err: fmt.Errorf("transmit image %q: %w", ap, err)}
		}
		return UpdateMsg{Path: p, Gen: gen, inner: transmittedMsg{path: p}}
	}
}

func TransmitAnimationCmd(m Model) tea.Cmd {
	p, ap, ids, c, r, sz, fsys, gen := m.path, m.absPath, append([]uint32(nil), m.anim.frameIDs...), m.cols, m.rows, m.cellSize, m.fs, m.gen
	return func() tea.Msg {
		data, errMsg := readImageFile(fsys, p, ap, gen)
		if errMsg != nil {
			return errMsg
		}
		gif, err := imagekit.DecodeGIF(data)
		if err != nil {
			return ErrorMsg{Path: p, Gen: gen, Err: fmt.Errorf("decode gif %q: %w", ap, err)}
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
				return ErrorMsg{Path: p, Gen: gen, Err: fmt.Errorf("encode frame %d of %q: %w", i, ap, encErr)}
			}
			seq.WriteString(s)
		}
		if err := writeTTY(seq.String()); err != nil {
			return ErrorMsg{Path: p, Gen: gen, Err: fmt.Errorf("transmit animation %q: %w", ap, err)}
		}
		return UpdateMsg{Path: p, Gen: gen, inner: transmittedMsg{path: p}}
	}
}

func EncodeITerm2Cmd(m Model) tea.Cmd {
	p, ap, c, r, sz, fsys, gen := m.path, m.absPath, m.cols, m.rows, m.cellSize, m.fs, m.gen
	return func() tea.Msg {
		dec, errMsg := readStill(fsys, p, ap, gen)
		if errMsg != nil {
			return errMsg
		}
		tw, th := imagekit.FitBox(dec.Width, dec.Height, c*sz.W, r*sz.H)
		resized := imagekit.Resize(dec.Image, tw, th)
		slices, err := imagekit.EncodeITerm2Rows(resized, c, r, sz)
		if err != nil {
			return ErrorMsg{Path: p, Gen: gen, Err: fmt.Errorf("encode iterm2 %q: %w", ap, err)}
		}
		return UpdateMsg{Path: p, Gen: gen, inner: encodedMsg{path: p, slices: slices}}
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

// DeleteCmd frees this image's terminal-side pixel memory: the base ID plus
// every allocated animation frame ID, batched into one write. Kitty only —
// iTerm2's inline protocol holds no server-side image state to free (its
// footprint is erased by overwriting cells, §3.1 in the markdownedit
// placement pipeline). Called on despawn (M6, syncImageSet) so a deleted or
// undone embed doesn't leak terminal pixel memory for the rest of the
// session. Fire-and-forget, like DeleteAllCmd.
func (m Model) DeleteCmd() tea.Cmd {
	if !m.termCaps.SupportsKittyGraphics() {
		return nil
	}
	return deleteIDsCmd(append([]uint32{m.id}, m.anim.frameIDs...))
}

// DeleteFramesCmd frees only this instance's allocated animation frame IDs —
// NOT the base ID. Called on an mtime respawn (syncImageSet): the respawn
// reuses the base ID and its retransmit overwrites that ID's terminal-side
// data in place, so deleting it here would race the concurrent new transmit
// (both are async Cmds with no ordering guarantee) and could erase the fresh
// image; the old frame IDs are never reused, so deleting them is safe
// whenever the write lands. Nil when there is nothing to free.
func (m Model) DeleteFramesCmd() tea.Cmd {
	if !m.termCaps.SupportsKittyGraphics() || len(m.anim.frameIDs) == 0 {
		return nil
	}
	return deleteIDsCmd(append([]uint32(nil), m.anim.frameIDs...))
}

// deleteIDsCmd batches one Kitty delete escape per ID into a single TTY
// write. Fire-and-forget, like DeleteAllCmd.
func deleteIDsCmd(ids []uint32) tea.Cmd {
	return func() tea.Msg {
		var seq strings.Builder
		for _, id := range ids {
			seq.WriteString(imagekit.EncodeDelete(id))
		}
		_ = writeTTY(seq.String())
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
