# WP17 — Terminal Capability Detection & Image Support

## Scope

Terminal feature probing, image paste/copy, terminal graphics rendering

## Dependencies

- WP8 (editor component — Update handles image ClipboardContentMsg)
- WP14 (markdown preview — image tokens in SyntaxMap)
- WP15 (clipboard — ClipboardContentMsg with ImageData/MIMEType fields)

## Deliverables

### Terminal Capability Detection

At startup, probe terminal for:
- **Kitty keyboard protocol** — for accurate modifier reporting (Cmd key fidelity)
- **Graphics protocol** — Kitty/iTerm2/WezTerm inline images
- **OSC-52** — clipboard access via terminal escape
- **Bracketed paste** — distinguish pasted text from typed text

Detection methods:
- `$TERM_PROGRAM` / `$TERM` environment variables
- Escape sequence probing (send query, read response with timeout)
- `terminfo` database lookup

Store capabilities in a `TermCaps` struct passed to editor at construction.

### Image Paste

When clipboard contains image data (detected by MIME type):
1. `ClipboardContentMsg` carries image bytes + MIME type
2. Editor saves image to configurable assets directory
3. Generate collision-safe filename (timestamp + hash)
4. Insert markdown reference: `![](relative/path/to/image.png)`
5. On next `syncDisplay`, SyntaxMap parses image token

### Image Copy

When cursor is on a rendered image token with no text selection:
1. `clipboard.copy` detects image AST node at cursor position
2. Read image file from path in token
3. Encode via terminal graphics protocol (Kitty/iTerm2)
4. Place on OS clipboard

### Image Rendering in Display

In SyntaxMap:
- `TokenImage` with `Rendered` state → placeholder for terminal graphics
- Actual rendering happens in `View()` using detected graphics protocol
- Fallback (no graphics support): show `[image: alt-text]` styled text

### Fallback Behavior

For unsupported terminals:
- No graphics → images shown as `[image: filename]`
- No OSC-52 → clipboard via `pbcopy`/`xclip` (already in WP15)
- No Kitty keyboard → standard modifier reporting (some Cmd combos may not work)
- Document fallback bindings for terminals without full modifier support

### TermCaps Struct

```go
type TermCaps struct {
    GraphicsProtocol GraphicsProto  // None, Kitty, ITerm2, WezTerm
    KittyKeyboard    bool
    OSC52Clipboard   bool
    BracketedPaste   bool
    TrueColor        bool
}

type GraphicsProto int
const (
    GraphicsNone GraphicsProto = iota
    GraphicsKitty
    GraphicsITerm2
    GraphicsWezTerm
)

func DetectCapabilities() TermCaps
```

### Tests

- Capability detection with mocked env vars
- Image paste: mock clipboard with image data → file written, markdown inserted
- Image copy: cursor on image token → correct file read
- Fallback: no graphics → text placeholder rendered

## Constraints

- Detection must be non-blocking (timeout on escape sequence probing)
- Image files saved with safe filenames (no path traversal)
- Relative paths in markdown references (portable)
- Under 500 LoC per file

## Verification

```bash
go build ./...
```

Manual testing:
- In Kitty: paste image → file saved in assets, `![](path)` inserted
- In Kitty: cursor on image → rendered inline
- In basic terminal (e.g., Terminal.app): graceful fallback, no crash
- Image copy in supported terminal → image on clipboard
