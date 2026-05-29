package editor

import (
	"errors"
	"fmt"
	"os"
	"time"

	tea "charm.land/bubbletea/v2"
	uv "github.com/charmbracelet/ultraviolet"

	"rune/pkg/imagekit"
)

// ImageDecodedMsg reports that a document image was decoded and measured. It
// carries dimensions and (for animated GIFs) frame metadata only — never
// pixels (pixels stay inside Cmd closures).
type ImageDecodedMsg struct {
	Path       string // raw markdown destination (registry key)
	Cols, Rows int    // cell footprint
	PxW, PxH   int    // decoded pixel dimensions
	Mtime      int64  // source modtime at decode time

	// Animation metadata (set only for animated GIFs).
	Animated   bool
	FrameCount int
	Delays     []time.Duration
	LoopCount  int
}

// ImageTransmittedMsg reports that an image's pixels were transmitted to the
// terminal and the image is now live.
type ImageTransmittedMsg struct {
	Path string
}

// ImageDecodeErrorMsg reports a decode/read failure for a document image.
type ImageDecodeErrorMsg struct {
	Path string
	Err  error
}

// ImageTransmitErrorMsg reports a transmit failure for a document image.
type ImageTransmitErrorMsg struct {
	Path string
	Err  error
}

// DecodeImageCmd reads, decodes, and measures an image off the main goroutine,
// returning an ImageDecodedMsg with its cell footprint. Pixels are discarded
// here; they are re-read for transmission (keeping the Model pixel-free).
func DecodeImageCmd(path, absPath string, mtime int64, maxCols, maxRows int, cs imagekit.CellSize) tea.Cmd {
	p, ap, mt, mc, mr, c := path, absPath, mtime, maxCols, maxRows, cs
	return func() tea.Msg {
		data, err := os.ReadFile(ap)
		if err != nil {
			return ImageDecodeErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}

		// Animated GIF: decode all frames for metadata; fall through to still
		// decode for single-frame GIFs and every other format.
		if imagekit.SniffFormat(data) == "gif" {
			if anim, aerr := imagekit.DecodeGIF(data); aerr == nil {
				cols, rows := imagekit.FitCells(anim.Width, anim.Height, mc, mr, c)
				return ImageDecodedMsg{
					Path: p, Cols: cols, Rows: rows,
					PxW: anim.Width, PxH: anim.Height, Mtime: mt,
					Animated: true, FrameCount: len(anim.Frames),
					Delays: anim.Delays, LoopCount: anim.LoopCount,
				}
			} else if !errors.Is(aerr, imagekit.ErrNotAnimated) {
				return ImageDecodeErrorMsg{Path: p, Err: aerr}
			}
		}

		dec, err := imagekit.DecodeStill(data)
		if err != nil {
			return ImageDecodeErrorMsg{Path: p, Err: err}
		}
		cols, rows := imagekit.FitCells(dec.Width, dec.Height, mc, mr, c)
		return ImageDecodedMsg{Path: p, Cols: cols, Rows: rows, PxW: dec.Width, PxH: dec.Height, Mtime: mt}
	}
}

// TransmitAnimationCmd re-decodes an animated GIF and transmits each composited
// frame as its own Kitty image (frameIDs[i]), so the editor can swap frames by
// changing which ID the placeholder cells reference. Reports
// ImageTransmittedMsg on success. Frame pixels live only within this closure.
func TransmitAnimationCmd(path, absPath string, frameIDs []uint32, cols, rows int, cs imagekit.CellSize) tea.Cmd {
	p, ap, ids, c, r, sz := path, absPath, append([]uint32(nil), frameIDs...), cols, rows, cs
	return func() tea.Msg {
		data, err := os.ReadFile(ap)
		if err != nil {
			return ImageTransmitErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}
		anim, err := imagekit.DecodeGIF(data)
		if err != nil {
			return ImageTransmitErrorMsg{Path: p, Err: fmt.Errorf("decode gif %q: %w", ap, err)}
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
				return ImageTransmitErrorMsg{Path: p, Err: fmt.Errorf("encode frame %d of %q: %w", i, ap, encErr)}
			}
			seq += s
		}
		if err := writeTTY(seq); err != nil {
			return ImageTransmitErrorMsg{Path: p, Err: fmt.Errorf("transmit animation %q: %w", ap, err)}
		}
		return ImageTransmittedMsg{Path: p}
	}
}

// TransmitImageCmd re-reads, decodes, resizes, and transmits an image to the
// terminal via Kitty virtual placement, then reports ImageTransmittedMsg. The
// resized pixels live only within this closure.
func TransmitImageCmd(path, absPath string, id uint32, cols, rows int, cs imagekit.CellSize) tea.Cmd {
	p, ap, theID, c, r, sz := path, absPath, id, cols, rows, cs
	return func() tea.Msg {
		data, err := os.ReadFile(ap)
		if err != nil {
			return ImageTransmitErrorMsg{Path: p, Err: fmt.Errorf("read image %q: %w", ap, err)}
		}
		dec, err := imagekit.DecodeStill(data)
		if err != nil {
			return ImageTransmitErrorMsg{Path: p, Err: err}
		}
		// Downscale toward the cell box's pixel size to bound transmit cost.
		tw, th := imagekit.FitBox(dec.Width, dec.Height, c*sz.W, r*sz.H)
		resized := imagekit.Resize(dec.Image, tw, th)
		seq, err := imagekit.EncodeTransmit(resized, theID, c, r)
		if err != nil {
			return ImageTransmitErrorMsg{Path: p, Err: fmt.Errorf("encode image %q: %w", ap, err)}
		}
		if err := writeTTY(seq); err != nil {
			return ImageTransmitErrorMsg{Path: p, Err: fmt.Errorf("transmit image %q: %w", ap, err)}
		}
		return ImageTransmittedMsg{Path: p}
	}
}

// DeleteImagesCmd deletes the given image IDs from the terminal. It is
// fire-and-forget: it returns no message.
func DeleteImagesCmd(ids []uint32) tea.Cmd {
	captured := append([]uint32(nil), ids...)
	return func() tea.Msg {
		if len(captured) == 0 {
			return nil
		}
		var seq string
		for _, id := range captured {
			seq += imagekit.EncodeDelete(id)
		}
		// fire-and-forget: a tty write failure on cleanup is not actionable.
		_ = writeTTY(seq)
		return nil
	}
}

// DeleteAllImagesCmd is the Model-method form used by the page to clear images
// before quitting.
func (m Model) DeleteAllImagesCmd() tea.Cmd { return DeleteAllImagesCmd() }

// DeleteAllImagesCmd deletes all images from the terminal. Used on quit; run it
// via tea.Sequence before tea.Quit so the delete flushes before exit.
func DeleteAllImagesCmd() tea.Cmd {
	return func() tea.Msg {
		// fire-and-forget: best-effort cleanup on exit.
		_ = writeTTY(imagekit.EncodeDeleteAll())
		return nil
	}
}

// writeTTY writes a raw escape sequence to the terminal's output file. Bubble
// Tea owns stdout, so out-of-band graphics bytes go straight to the tty.
func writeTTY(seq string) error {
	if seq == "" {
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
	if _, err := outTty.WriteString(seq); err != nil {
		return fmt.Errorf("write tty: %w", err)
	}
	return nil
}
