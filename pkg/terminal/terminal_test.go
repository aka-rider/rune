package terminal

import "testing"

// mockProber implements Prober for deterministic testing.
type mockProber struct {
	vars map[string]string
}

func (m mockProber) Env(key string) string {
	return m.vars[key]
}

func TestDetect_KittyFromTermProgram(t *testing.T) {
	p := mockProber{vars: map[string]string{"TERM_PROGRAM": "kitty"}}
	caps := DetectWithProber(p)

	if caps.GraphicsProtocol != GraphicsKitty {
		t.Errorf("expected GraphicsKitty, got %v", caps.GraphicsProtocol)
	}
	if !caps.KittyKeyboard {
		t.Error("expected KittyKeyboard=true for kitty")
	}
	if !caps.OSC52Clipboard {
		t.Error("expected OSC52=true for kitty")
	}
}

func TestDetect_ITermFromTermProgram(t *testing.T) {
	p := mockProber{vars: map[string]string{"TERM_PROGRAM": "iTerm.app"}}
	caps := DetectWithProber(p)

	if caps.GraphicsProtocol != GraphicsITerm2 {
		t.Errorf("expected GraphicsITerm2, got %v", caps.GraphicsProtocol)
	}
	if !caps.OSC52Clipboard {
		t.Error("expected OSC52=true for iTerm")
	}
}

func TestDetect_WezTermFromTermProgram(t *testing.T) {
	p := mockProber{vars: map[string]string{"TERM_PROGRAM": "WezTerm"}}
	caps := DetectWithProber(p)

	if caps.GraphicsProtocol != GraphicsWezTerm {
		t.Errorf("expected GraphicsWezTerm, got %v", caps.GraphicsProtocol)
	}
	if !caps.OSC52Clipboard {
		t.Error("expected OSC52=true for WezTerm")
	}
}

func TestDetect_KittyWindowIDFallback(t *testing.T) {
	// No TERM_PROGRAM but KITTY_WINDOW_ID present
	p := mockProber{vars: map[string]string{"KITTY_WINDOW_ID": "1"}}
	caps := DetectWithProber(p)

	if caps.GraphicsProtocol != GraphicsKitty {
		t.Errorf("expected GraphicsKitty from KITTY_WINDOW_ID, got %v", caps.GraphicsProtocol)
	}
	if !caps.KittyKeyboard {
		t.Error("expected KittyKeyboard=true from KITTY_WINDOW_ID")
	}
}

func TestDetect_UnknownTerminal_SafeDefaults(t *testing.T) {
	p := mockProber{vars: map[string]string{
		"TERM_PROGRAM": "unknown-terminal",
		"TERM":         "xterm",
	}}
	caps := DetectWithProber(p)

	if caps.GraphicsProtocol != GraphicsNone {
		t.Errorf("expected GraphicsNone for unknown terminal, got %v", caps.GraphicsProtocol)
	}
	if caps.KittyKeyboard {
		t.Error("expected KittyKeyboard=false for unknown terminal")
	}
	if caps.OSC52Clipboard {
		t.Error("expected OSC52=false for unknown terminal")
	}
	if !caps.BracketedPaste {
		t.Error("expected BracketedPaste=true (safe default)")
	}
}

func TestDetect_EmptyEnv_SafeDefaults(t *testing.T) {
	p := mockProber{vars: map[string]string{}}
	caps := DetectWithProber(p)

	if caps.GraphicsProtocol != GraphicsNone {
		t.Errorf("expected GraphicsNone for empty env, got %v", caps.GraphicsProtocol)
	}
	if caps.SupportsGraphics() {
		t.Error("SupportsGraphics should be false for empty env")
	}
	if !caps.BracketedPaste {
		t.Error("BracketedPaste should default to true")
	}
	if caps.TrueColor {
		t.Error("TrueColor should be false without COLORTERM")
	}
}

func TestDetect_TrueColor_Colorterm(t *testing.T) {
	p := mockProber{vars: map[string]string{"COLORTERM": "truecolor"}}
	caps := DetectWithProber(p)

	if !caps.TrueColor {
		t.Error("expected TrueColor=true with COLORTERM=truecolor")
	}
}

func TestDetect_TrueColor_24bit(t *testing.T) {
	p := mockProber{vars: map[string]string{"COLORTERM": "24bit"}}
	caps := DetectWithProber(p)

	if !caps.TrueColor {
		t.Error("expected TrueColor=true with COLORTERM=24bit")
	}
}

func TestDetect_TrueColor_Term256(t *testing.T) {
	p := mockProber{vars: map[string]string{"TERM": "xterm-256color"}}
	caps := DetectWithProber(p)

	if !caps.TrueColor {
		t.Error("expected TrueColor=true with TERM containing 256color")
	}
}

func TestDetect_OSC52_Tmux(t *testing.T) {
	p := mockProber{vars: map[string]string{"TMUX": "/tmp/tmux-1000/default,12345,0"}}
	caps := DetectWithProber(p)

	if !caps.OSC52Clipboard {
		t.Error("expected OSC52=true with TMUX set")
	}
}

func TestDetect_OSC52_Alacritty(t *testing.T) {
	p := mockProber{vars: map[string]string{"TERM_PROGRAM": "alacritty"}}
	caps := DetectWithProber(p)

	if !caps.OSC52Clipboard {
		t.Error("expected OSC52=true for alacritty")
	}
	// Alacritty has no image graphics
	if caps.GraphicsProtocol != GraphicsNone {
		t.Errorf("expected GraphicsNone for alacritty, got %v", caps.GraphicsProtocol)
	}
}

func TestSupportsGraphics(t *testing.T) {
	tests := []struct {
		name   string
		proto  GraphicsProto
		expect bool
	}{
		{"None", GraphicsNone, false},
		{"Kitty", GraphicsKitty, true},
		{"ITerm2", GraphicsITerm2, true},
		{"WezTerm", GraphicsWezTerm, true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			caps := TermCaps{GraphicsProtocol: tt.proto}
			if caps.SupportsGraphics() != tt.expect {
				t.Errorf("SupportsGraphics()=%v, want %v", caps.SupportsGraphics(), tt.expect)
			}
		})
	}
}
