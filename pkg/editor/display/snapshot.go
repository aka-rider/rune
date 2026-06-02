package display

import (
	"github.com/mattn/go-runewidth"
)

// LinkRole classifies a link-like span by how it should be treated for
// rendering and click handling, unifying standard markdown and Obsidian wiki
// syntaxes onto one set of behaviors.
type LinkRole int

const (
	LinkRoleNone LinkRole = iota
	LinkRoleNavigable
	LinkRoleImage
)

// linkRoleFor is the single source of truth mapping a token kind (plus the
// wiki-link image flag) to its LinkRole. Both DisplaySpan and SyntaxSpan expose
// it via their LinkRole() methods.
func linkRoleFor(kind TokenKind, wikiIsImage bool) LinkRole {
	switch kind {
	case TokenImage:
		return LinkRoleImage
	case TokenWikiLink:
		if wikiIsImage {
			return LinkRoleImage
		}
		return LinkRoleNavigable
	case TokenLink:
		return LinkRoleNavigable
	default:
		return LinkRoleNone
	}
}

type DisplaySpan struct {
	Text         string
	Kind         TokenKind
	State        RevealState
	BufferStart  int
	BufferEnd    int
	CellMap      []CellMapping // per-visual-byte buffer offsets for Rendered spans; nil for Revealed
	Language     string
	BlockID      int
	BlockStart   int
	BlockEnd     int
	AltText      string
	ImagePath    string
	EmbedRef     string
	CalloutKind  string
	HeadingLevel  int
	TableRole     TableRoleKind
	TableLayout   TableLayoutKind // layout state (grid/wrapped/pivoted)
	// Wiki link metadata (set for TokenWikiLink spans)
	WikiLinkTarget  string // resolved file path for wiki links
	WikiLinkIsImage bool   // true for embedded images ![[image.png]]
	LinkURL         string // link destination for TokenLink/TokenImage spans
}

// LinkRole reports how this span should be treated as a link/embed.
func (s DisplaySpan) LinkRole() LinkRole { return linkRoleFor(s.Kind, s.WikiLinkIsImage) }

type DisplayLine struct {
	Spans     []DisplaySpan
	ModelLine int
	WrapIndex int

	// Image row reservation, populated only by ExpandImageRows for standalone
	// image lines. Zero-valued for all other lines (BuildSnapshot never sets
	// these). When ImagePath != "", this line is part of a reserved image
	// block: ImageRowIndex is its 0-based row within the block, ImageRowCount
	// is the block height, and ImageCols is the block width in cells.
	ImagePath     string
	ImageRowIndex int
	ImageRowCount int
	ImageCols     int
}

type DisplaySnapshot struct {
	Lines          []DisplayLine
	TotalRows      int
	rowToModelLine []int
	lineToFirstRow []int
}

func BuildSnapshot(ws WrapSnapshot) DisplaySnapshot {
	var dlines []DisplayLine
	rowToModelLine := make([]int, ws.TotalRows)
	lineToFirstRow := make([]int, len(ws.lineToFirstRow))
	copy(lineToFirstRow, ws.lineToFirstRow)

	for i, seg := range ws.Segments {
		var spans []DisplaySpan
		for _, s := range seg.Spans {
			spans = append(spans, DisplaySpan{
				Text:            s.Text,
				Kind:            s.Kind,
				State:           s.State,
				BufferStart:     s.BufferStart,
				BufferEnd:       s.BufferEnd,
				CellMap:         s.CellMap,
				Language:        s.Language,
				BlockID:         s.BlockID,
				BlockStart:      s.BlockStart,
				BlockEnd:        s.BlockEnd,
				AltText:         s.AltText,
				HeadingLevel:    s.HeadingLevel,
				TableRole:       s.TableRole,
				TableLayout:     s.TableLayout,
				ImagePath:       s.ImagePath,
				EmbedRef:        s.EmbedRef,
				CalloutKind:     s.CalloutKind,
				WikiLinkTarget:  s.WikiLinkTarget,
				WikiLinkIsImage: s.WikiLinkIsImage,
				LinkURL:         s.LinkURL,
			})
		}
		dlines = append(dlines, DisplayLine{
			Spans:     spans,
			ModelLine: seg.ModelLine,
			WrapIndex: seg.WrapIndex,
		})
		rowToModelLine[i] = seg.ModelLine
	}

	return DisplaySnapshot{
		Lines:          dlines,
		TotalRows:      ws.TotalRows,
		rowToModelLine: rowToModelLine,
		lineToFirstRow: lineToFirstRow,
	}
}

func (ds DisplaySnapshot) Slice(topRow, height int) []DisplayLine {
	if topRow < 0 {
		topRow = 0
	}
	if topRow >= len(ds.Lines) {
		return nil
	}
	end := topRow + height
	if end > len(ds.Lines) {
		end = len(ds.Lines)
	}

	// Create a new slice backed by the same array
	result := make([]DisplayLine, end-topRow)
	copy(result, ds.Lines[topRow:end])
	return result
}

func sliceSpanStr(text string, scrollCol, width int, startW int) (string, int) {
	curr := startW
	startByte := -1
	endByte := -1

	for i, r := range text {
		rw := runewidth.RuneWidth(r)
		if r == '\t' {
			rw = 4 - (curr % 4)
		}

		if curr+rw > scrollCol && startByte == -1 {
			startByte = i
		}

		if curr+rw > scrollCol+width && endByte == -1 {
			endByte = i
			break
		}
		curr += rw
	}

	if startByte == -1 {
		return "", curr
	}
	if endByte == -1 {
		endByte = len(text)
	}

	return text[startByte:endByte], curr
}

func (ds DisplaySnapshot) SliceH(lines []DisplayLine, scrollCol, width int) []DisplayLine {
	if scrollCol <= 0 && width <= 0 {
		return lines
	}

	var result []DisplayLine
	for _, l := range lines {
		dl := DisplayLine{
			ModelLine: l.ModelLine,
			WrapIndex: l.WrapIndex,
		}

		currW := 0
		for _, s := range l.Spans {
			if currW > scrollCol+width {
				break
			}

			// Preserve spans with empty text — they carry semantic metadata
			// (e.g., language labels for code fences).
			if s.Text == "" {
				dl.Spans = append(dl.Spans, s)
				continue
			}

			spanText, nextW := sliceSpanStr(s.Text, scrollCol, width, currW)
			if len(spanText) > 0 {
				newS := s
				newS.Text = spanText
				// We don't recalculate BufferStart/End exactly here because it's just for display rendering
				dl.Spans = append(dl.Spans, newS)
			}
			currW = nextW
		}
		result = append(result, dl)
	}
	return result
}

func (ds DisplaySnapshot) ModelLineToFirstRow(line int) int {
	if line < 0 || line >= len(ds.lineToFirstRow) {
		return 0
	}
	return ds.lineToFirstRow[line]
}

func (ds DisplaySnapshot) RowToModelLine(row int) int {
	if row < 0 || row >= len(ds.rowToModelLine) {
		return 0
	}
	return ds.rowToModelLine[row]
}
