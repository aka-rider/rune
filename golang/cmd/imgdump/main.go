// imgdump dumps the reference imagekit package's outputs as JSON goldens so
// the Rust port (crates/rune-image) can assert byte-for-byte parity.
//
// Usage: go run ./cmd/imgdump <subcommand> [args...]
//
// Subcommands:
//
//	pure                          - dump FitBox/FitCells/AllocID/ClampDelay/Diacritic cases
//	encode <asset> <id> <cols> <rows> - decode+fit+resize+transmit-encode one asset
//	delete <id>                   - the delete-one escape string
//	delete-all                    - the delete-all escape string
package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"strconv"

	"rune/pkg/imagekit"
)

func main() {
	if len(os.Args) < 2 {
		fatalf("usage: imgdump <pure|encode|delete|delete-all> [args...]")
	}

	switch os.Args[1] {
	case "pure":
		dumpPure()
	case "encode":
		dumpEncode(os.Args[2:])
	case "delete":
		dumpDelete(os.Args[2:])
	case "delete-all":
		dumpDeleteAll()
	default:
		fatalf("unknown subcommand %q", os.Args[1])
	}
}

func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}

// ── pure ─────────────────────────────────────────────────────────────────

type fitBoxCase struct {
	Input  fitBoxInput  `json:"input"`
	Output fitBoxOutput `json:"output"`
}

type fitBoxInput struct {
	SrcW int `json:"src_w"`
	SrcH int `json:"src_h"`
	MaxW int `json:"max_w"`
	MaxH int `json:"max_h"`
}

type fitBoxOutput struct {
	W int `json:"w"`
	H int `json:"h"`
}

type fitCellsCase struct {
	Input  fitCellsInput  `json:"input"`
	Output fitCellsOutput `json:"output"`
}

type fitCellsInput struct {
	PxW     int `json:"px_w"`
	PxH     int `json:"px_h"`
	MaxCols int `json:"max_cols"`
	MaxRows int `json:"max_rows"`
	CellW   int `json:"cell_w"`
	CellH   int `json:"cell_h"`
}

type fitCellsOutput struct {
	Cols int `json:"cols"`
	Rows int `json:"rows"`
}

type allocIDCase struct {
	Input  allocIDInput  `json:"input"`
	Output allocIDOutput `json:"output"`
}

type allocIDInput struct {
	Path string `json:"path"`
}

type allocIDOutput struct {
	ID uint32 `json:"id"`
}

type clampDelayCase struct {
	Input  clampDelayInput  `json:"input"`
	Output clampDelayOutput `json:"output"`
}

type clampDelayInput struct {
	Hundredths int64 `json:"hundredths"`
}

type clampDelayOutput struct {
	Millis int64 `json:"millis"`
}

type diacriticCase struct {
	Input  diacriticInput  `json:"input"`
	Output diacriticOutput `json:"output"`
}

type diacriticInput struct {
	Index int `json:"index"`
}

type diacriticOutput struct {
	Codepoint int32 `json:"codepoint"`
}

type pureDump struct {
	FitBox     []fitBoxCase     `json:"fit_box"`
	FitCells   []fitCellsCase   `json:"fit_cells"`
	AllocID    []allocIDCase    `json:"alloc_id"`
	ClampDelay []clampDelayCase `json:"clamp_delay"`
	Diacritic  []diacriticCase  `json:"diacritic"`
}

func dumpPure() {
	dump := pureDump{
		FitBox:     fitBoxCases(),
		FitCells:   fitCellsCases(),
		AllocID:    allocIDCases(),
		ClampDelay: clampDelayCases(),
		Diacritic:  diacriticCases(),
	}
	writeJSON(dump)
}

func fitBoxCases() []fitBoxCase {
	inputs := []fitBoxInput{
		// From imagekit's TestFitBox.
		{10, 10, 100, 100},
		{200, 100, 100, 100},
		{100, 200, 100, 100},
		{400, 400, 80, 80},
		{0, 10, 100, 100},
		// Truncating-scale boundary spread.
		{3, 3, 2, 2},
		{7, 5, 3, 100},
		{5, 7, 100, 3},
		{1, 1, 1, 1},
		{1000, 1, 3, 1000},
		{1, 1000, 1000, 3},
		{17, 33, 10, 10},
	}
	cases := make([]fitBoxCase, 0, len(inputs))
	for _, in := range inputs {
		w, h := imagekit.FitBox(in.SrcW, in.SrcH, in.MaxW, in.MaxH)
		cases = append(cases, fitBoxCase{Input: in, Output: fitBoxOutput{W: w, H: h}})
	}
	return cases
}

func fitCellsCases() []fitCellsCase {
	def := imagekit.DefaultCellSize()
	inputs := []struct {
		px, py, mc, mr, cw, ch int
	}{
		// From imagekit's TestFitCells.
		{80, 160, 100, 100, def.W, def.H},
		{800, 1600, 20, 100, def.W, def.H},
		{0, 0, 10, 10, def.W, def.H},
		// Ceil-div boundary spread: pixel sizes not multiples of the cell size.
		{17, 33, 10, 10, 8, 16},
		{81, 161, 10, 3, 8, 16},
		{1, 1, 100, 100, 8, 16},
		{64, 48, 3, 8, 8, 16},
		{40, 40, 5, 3, 8, 16},
		{80, 60, 10, 4, 8, 16},
		{48, 48, 6, 3, 8, 16},
		{64, 48, 80, 40, 8, 16},
	}
	cases := make([]fitCellsCase, 0, len(inputs))
	for _, in := range inputs {
		cs := imagekit.CellSize{W: in.cw, H: in.ch}
		cols, rows := imagekit.FitCells(in.px, in.py, in.mc, in.mr, cs)
		cases = append(cases, fitCellsCase{
			Input: fitCellsInput{
				PxW: in.px, PxH: in.py, MaxCols: in.mc, MaxRows: in.mr,
				CellW: in.cw, CellH: in.ch,
			},
			Output: fitCellsOutput{Cols: cols, Rows: rows},
		})
	}
	return cases
}

func allocIDCases() []allocIDCase {
	paths := []string{
		"/abs/path/to/image.png",
		"/abs/path/to/image.png", // repeated: determinism
		"/other.png",
		"",
		"/testdata/assets/x.png",
		"/testdata/assets/y.png",
		"/testdata/assets/photo.jpg",
		"/testdata/assets/anim.gif",
		"/testdata/assets/vector.svg",
	}
	cases := make([]allocIDCase, 0, len(paths))
	for _, p := range paths {
		cases = append(cases, allocIDCase{
			Input:  allocIDInput{Path: p},
			Output: allocIDOutput{ID: imagekit.AllocID(p)},
		})
	}
	return cases
}

func clampDelayCases() []clampDelayCase {
	inputs := []int64{-5, -1, 0, 1, 4, 5, 6, 10, 50, 100, 1000}
	cases := make([]clampDelayCase, 0, len(inputs))
	for _, h := range inputs {
		d := imagekit.ClampDelay(int(h))
		cases = append(cases, clampDelayCase{
			Input:  clampDelayInput{Hundredths: h},
			Output: clampDelayOutput{Millis: d.Milliseconds()},
		})
	}
	return cases
}

func diacriticCases() []diacriticCase {
	cases := make([]diacriticCase, 0, 299)
	for i := 0; i <= 296; i++ {
		cases = append(cases, diacriticCase{
			Input:  diacriticInput{Index: i},
			Output: diacriticOutput{Codepoint: int32(imagekit.Diacritic(i))},
		})
	}
	for _, i := range []int{-1, 297} {
		cases = append(cases, diacriticCase{
			Input:  diacriticInput{Index: i},
			Output: diacriticOutput{Codepoint: int32(imagekit.Diacritic(i))},
		})
	}
	return cases
}

// ── encode ───────────────────────────────────────────────────────────────

type apcRecord struct {
	Options    string `json:"options"`
	PayloadB64 string `json:"payload_b64"`
}

type encodeDump struct {
	Asset          string      `json:"asset"`
	ID             uint32      `json:"id"`
	RequestedCols  int         `json:"requested_cols"`
	RequestedRows  int         `json:"requested_rows"`
	SourceWidth    int         `json:"source_width"`
	SourceHeight   int         `json:"source_height"`
	ResizedWidth   int         `json:"resized_width"`
	ResizedHeight  int         `json:"resized_height"`
	APCs           []apcRecord `json:"apcs"`
	ResizedRGBAB64 string      `json:"resized_rgba_b64"`
}

func dumpEncode(args []string) {
	if len(args) != 4 {
		fatalf("usage: imgdump encode <asset> <id> <cols> <rows>")
	}
	asset := args[0]
	id, err := strconv.ParseUint(args[1], 10, 32)
	if err != nil {
		fatalf("bad id %q: %v", args[1], err)
	}
	cols, err := strconv.Atoi(args[2])
	if err != nil {
		fatalf("bad cols %q: %v", args[2], err)
	}
	rows, err := strconv.Atoi(args[3])
	if err != nil {
		fatalf("bad rows %q: %v", args[3], err)
	}

	data, err := os.ReadFile(asset)
	if err != nil {
		fatalf("read %s: %v", asset, err)
	}

	decoded, err := imagekit.DecodeStill(data)
	if err != nil {
		fatalf("decode %s: %v", asset, err)
	}

	bounds := decoded.Image.Bounds()
	srcW, srcH := bounds.Dx(), bounds.Dy()

	fitW, fitH := imagekit.FitBox(srcW, srcH, cols*8, rows*16)
	resized := imagekit.Resize(decoded.Image, fitW, fitH)

	seq, err := imagekit.EncodeTransmit(resized, uint32(id), cols, rows)
	if err != nil {
		fatalf("encode transmit: %v", err)
	}

	apcs := splitAPCs(seq)

	rb := resized.Bounds()
	rgba := make([]byte, 0, rb.Dx()*rb.Dy()*4)
	for y := rb.Min.Y; y < rb.Max.Y; y++ {
		for x := rb.Min.X; x < rb.Max.X; x++ {
			r, g, b, a := resized.At(x, y).RGBA()
			rgba = append(rgba, byte(r>>8), byte(g>>8), byte(b>>8), byte(a>>8))
		}
	}

	dump := encodeDump{
		Asset:          asset,
		ID:             uint32(id),
		RequestedCols:  cols,
		RequestedRows:  rows,
		SourceWidth:    srcW,
		SourceHeight:   srcH,
		ResizedWidth:   rb.Dx(),
		ResizedHeight:  rb.Dy(),
		APCs:           apcs,
		ResizedRGBAB64: base64.StdEncoding.EncodeToString(rgba),
	}
	writeJSON(dump)
}

// splitAPCs splits a (possibly chunked) sequence of APC escapes
// ("\x1b_G" + options + [";" + payload] + "\x1b\\") into individual records.
func splitAPCs(seq string) []apcRecord {
	const (
		intro = "\x1b_G"
		outro = "\x1b\\"
	)
	var records []apcRecord
	rest := seq
	for len(rest) > 0 {
		if len(rest) < len(intro) || rest[:len(intro)] != intro {
			fatalf("malformed APC stream: missing introducer at %q", snippet(rest))
		}
		rest = rest[len(intro):]
		end := indexOf(rest, outro)
		if end < 0 {
			fatalf("malformed APC stream: missing terminator")
		}
		body := rest[:end]
		rest = rest[end+len(outro):]

		options := body
		payload := ""
		if semi := indexOf(body, ";"); semi >= 0 {
			options = body[:semi]
			payload = body[semi+1:]
		}
		// payload is already base64 text per the Kitty direct-transmission
		// wire format (see EncodeTransmit's doc comment) — no re-encoding.
		records = append(records, apcRecord{
			Options:    options,
			PayloadB64: payload,
		})
	}
	return records
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}

func snippet(s string) string {
	if len(s) > 40 {
		return s[:40]
	}
	return s
}

// ── delete / delete-all ─────────────────────────────────────────────────

type deleteDump struct {
	ID     uint32 `json:"id,omitempty"`
	Escape string `json:"escape"`
}

func dumpDelete(args []string) {
	if len(args) != 1 {
		fatalf("usage: imgdump delete <id>")
	}
	id, err := strconv.ParseUint(args[0], 10, 32)
	if err != nil {
		fatalf("bad id %q: %v", args[0], err)
	}
	writeJSON(deleteDump{ID: uint32(id), Escape: imagekit.EncodeDelete(uint32(id))})
}

func dumpDeleteAll() {
	writeJSON(deleteDump{Escape: imagekit.EncodeDeleteAll()})
}

// ── shared ───────────────────────────────────────────────────────────────

func writeJSON(v any) {
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		fatalf("encode json: %v", err)
	}
}
