package terminal

import (
	"os"
	"strings"
)

// GraphicsProto identifies the terminal graphics protocol available.
type GraphicsProto int

const (
	GraphicsNone    GraphicsProto = iota
	GraphicsKitty                 // Kitty graphics protocol
	GraphicsITerm2                // iTerm2 inline images
	GraphicsWezTerm               // WezTerm graphics (iTerm2-compatible)
)

// TermCaps describes detected terminal capabilities.
type TermCaps struct {
	GraphicsProtocol GraphicsProto
	KittyKeyboard    bool
	OSC52Clipboard   bool
	BracketedPaste   bool
	TrueColor        bool
}

// Prober abstracts terminal escape-sequence probing for testability.
// Production code uses EnvProber; tests can inject a mock.
type Prober interface {
	Env(key string) string
}

// EnvProber reads from the real environment.
type EnvProber struct{}

func (EnvProber) Env(key string) string { return os.Getenv(key) }

// DetectCapabilities probes the environment for terminal capabilities.
// Detection is non-blocking and safe — it only reads environment variables.
func DetectCapabilities() TermCaps {
	return DetectWithProber(EnvProber{})
}

// DetectWithProber performs capability detection using the given prober.
func DetectWithProber(p Prober) TermCaps {
	caps := TermCaps{
		BracketedPaste: true, // assumed for modern terminals
	}

	termProgram := strings.ToLower(p.Env("TERM_PROGRAM"))
	term := strings.ToLower(p.Env("TERM"))
	colorterm := strings.ToLower(p.Env("COLORTERM"))

	// Graphics protocol detection
	switch {
	case strings.Contains(termProgram, "kitty"):
		caps.GraphicsProtocol = GraphicsKitty
		caps.KittyKeyboard = true
	case strings.Contains(termProgram, "ghostty"), strings.Contains(term, "ghostty"):
		// Ghostty implements the Kitty graphics protocol including the
		// Unicode-placeholder virtual-placement extension.
		caps.GraphicsProtocol = GraphicsKitty
		caps.KittyKeyboard = true
	case strings.Contains(termProgram, "wezterm"):
		caps.GraphicsProtocol = GraphicsWezTerm
	case strings.Contains(termProgram, "iterm"):
		caps.GraphicsProtocol = GraphicsITerm2
	}

	// Kitty keyboard protocol from env
	if p.Env("KITTY_WINDOW_ID") != "" {
		caps.KittyKeyboard = true
		if caps.GraphicsProtocol == GraphicsNone {
			caps.GraphicsProtocol = GraphicsKitty
		}
	}

	// True color detection
	if colorterm == "truecolor" || colorterm == "24bit" {
		caps.TrueColor = true
	}
	if strings.Contains(term, "256color") || strings.Contains(term, "truecolor") {
		caps.TrueColor = true
	}

	// OSC52 clipboard — most modern terminals support this
	switch {
	case strings.Contains(termProgram, "kitty"),
		strings.Contains(termProgram, "ghostty"),
		strings.Contains(termProgram, "wezterm"),
		strings.Contains(termProgram, "iterm"),
		strings.Contains(termProgram, "tmux"),
		strings.Contains(termProgram, "alacritty"):
		caps.OSC52Clipboard = true
	}
	if p.Env("TMUX") != "" {
		caps.OSC52Clipboard = true
	}

	return caps
}

// SupportsGraphics returns true if any graphics protocol is available.
func (tc TermCaps) SupportsGraphics() bool {
	return tc.GraphicsProtocol != GraphicsNone
}

// SupportsKittyGraphics reports whether the terminal can render inline images
// via the Kitty graphics protocol's Unicode-placeholder virtual placement.
// Only Kitty and Ghostty qualify (both detected as GraphicsKitty), and only
// when truecolor is available — the image ID is carried in a 24-bit cell
// foreground color. WezTerm is intentionally excluded: its Kitty-graphics
// support historically lacks the Unicode-placeholder extension this relies on.
func (tc TermCaps) SupportsKittyGraphics() bool {
	return tc.GraphicsProtocol == GraphicsKitty && tc.TrueColor
}
