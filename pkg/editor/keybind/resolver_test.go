package keybind

import (
	"testing"
)

func TestResolverConstruction(t *testing.T) {
	tests := []struct {
		name      string
		bindings  []Binding
		wantError bool
	}{
		{
			name: "Valid resolver",
			bindings: []Binding{
				{Chords: []Chord{{Alt: true, Key: "up"}}, Command: "move"},
				{Chords: []Chord{{Alt: true, Shift: true, Key: "up"}}, Command: "clone"},
			},
			wantError: false,
		},
		{
			name: "Duplicate bindings exactly the same",
			bindings: []Binding{
				{Chords: []Chord{{Alt: true, Key: "up"}}, Command: "move"},
				{Chords: []Chord{{Alt: true, Key: "up"}}, Command: "move2"},
			},
			wantError: true, // Gate 1
		},
		{
			name: "Duplicate bindings with same context",
			bindings: []Binding{
				{Chords: []Chord{{Alt: true, Key: "up"}}, Command: "move", When: "editorFocused"},
				{Chords: []Chord{{Alt: true, Key: "up"}}, Command: "move2", When: "editorFocused"},
			},
			wantError: true, // Gate 1
		},
		{
			name: "Same chords, different context",
			bindings: []Binding{
				{Chords: []Chord{{Alt: true, Key: "up"}}, Command: "move", When: "editorFocused"},
				{Chords: []Chord{{Alt: true, Key: "up"}}, Command: "move2", When: "!editorFocused"},
			},
			wantError: false,
		},
		{
			name: "Malformed expression",
			bindings: []Binding{
				{Chords: []Chord{{Key: "a"}}, Command: "cmd", When: "editorFocused &&"},
			},
			wantError: true, // Malformed When
		},
		{
			name: "Unknown identifier",
			bindings: []Binding{
				{Chords: []Chord{{Key: "a"}}, Command: "cmd", When: "unknownID"},
			},
			wantError: true, // Unknown identifier
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := NewResolver(tt.bindings)
			if (err != nil) != tt.wantError {
				t.Fatalf("NewResolver() error = %v, wantError = %v", err, tt.wantError)
			}
		})
	}
}

func TestResolverMatch(t *testing.T) {
	bindings := []Binding{
		{Chords: []Chord{{Alt: true, Key: "right"}}, Command: "cursor.word-right"},
		{Chords: []Chord{{Ctrl: true, Key: "k"}, {Ctrl: true, Key: "v"}}, Command: "markdown.preview"},
		{Chords: []Chord{{Ctrl: true, Key: "k"}}, Command: "kill-line"},
		{Chords: []Chord{{Ctrl: true, Key: "k"}, {Ctrl: true, Key: "s"}}, Command: "save-all"},
		{Chords: []Chord{{Ctrl: true, Key: "c"}, {Key: "a"}}, Command: "context.a"},
		{Chords: []Chord{{Key: "enter"}}, Command: "insert.enter", When: "editorFocused"},
	}

	res, err := NewResolver(bindings)
	if err != nil {
		t.Fatalf("Failed to create resolver: %v", err)
	}

	t.Run("Modifier exactness", func(t *testing.T) {
		// Gate 2: Alt+Right -> cursor.word-right
		res2, result := res.Resolve(Chord{Alt: true, Key: "right"}, ResolverContext{})
		if result.Kind != ResultFound || result.Command != "cursor.word-right" {
			t.Errorf("Expected ResultFound with cursor.word-right, got %v: %v", result.Kind, result.Command)
		}
		_ = res2

		// Gate 2: Ctrl+Alt+Right -> no match
		_, result = res.Resolve(Chord{Ctrl: true, Alt: true, Key: "right"}, ResolverContext{})
		if result.Kind != ResultNoMatch {
			t.Errorf("Expected ResultNoMatch, got %v", result.Kind)
		}
	})

	t.Run("Multi-step chord", func(t *testing.T) {
		// Gate 3: Ctrl+K, Ctrl+V
		res2, result := res.Resolve(Chord{Ctrl: true, Key: "k"}, ResolverContext{})
		if result.Kind != ResultMoreChordsNeeded {
			t.Errorf("Expected ResultMoreChordsNeeded, got %v: %v", result.Kind, result.Command)
		}

		_, result = res2.Resolve(Chord{Ctrl: true, Key: "v"}, ResolverContext{})
		if result.Kind != ResultFound || result.Command != "markdown.preview" {
			t.Errorf("Expected ResultFound with markdown.preview, got %v: %v", result.Kind, result.Command)
		}
	})

	t.Run("Timeout resolution", func(t *testing.T) {
		// Gate 4: Timeout fires shortest match
		res2, result := res.Resolve(Chord{Ctrl: true, Key: "k"}, ResolverContext{})
		if result.Kind != ResultMoreChordsNeeded {
			t.Errorf("Expected ResultMoreChordsNeeded, got %v", result.Kind)
		}

		_, result = res2.ResolveTimeout()
		if result.Kind != ResultFound || result.Command != "kill-line" {
			t.Errorf("Expected ResultFound with kill-line, got %v: %v", result.Kind, result.Command)
		}
	})

	t.Run("Context predicate", func(t *testing.T) {
		// Gate 5: when: editorFocused
		_, result := res.Resolve(Chord{Key: "enter"}, ResolverContext{EditorFocused: false})
		if result.Kind != ResultNoMatch {
			t.Errorf("Expected ResultNoMatch from unfocused, got %v", result.Kind)
		}

		_, result = res.Resolve(Chord{Key: "enter"}, ResolverContext{EditorFocused: true})
		if result.Kind != ResultFound || result.Command != "insert.enter" {
			t.Errorf("Expected ResultFound with insert.enter, got %v: %v", result.Kind, result.Command)
		}
	})

	t.Run("No match resets pending", func(t *testing.T) {
		res2, result := res.Resolve(Chord{Ctrl: true, Key: "k"}, ResolverContext{})
		if result.Kind != ResultMoreChordsNeeded {
			t.Errorf("Expected ResultMoreChordsNeeded, got %v", result.Kind)
		}
		if !res2.InChordMode() {
			t.Errorf("Expected InChordMode to be true")
		}

		res3, result := res2.Resolve(Chord{Key: "x"}, ResolverContext{})
		if result.Kind != ResultNoMatch {
			t.Errorf("Expected ResultNoMatch, got %v", result.Kind)
		}
		if res3.InChordMode() {
			t.Errorf("Expected InChordMode to be false after no match")
		}
	})
}

func TestPredicateEvaluation(t *testing.T) {
	tests := []struct {
		name     string
		when     string
		ctx      ResolverContext
		expected bool
	}{
		{"And true", "editorFocused && hasSelection", ResolverContext{EditorFocused: true, HasSelection: true}, true},
		{"And false", "editorFocused && hasSelection", ResolverContext{EditorFocused: true, HasSelection: false}, false},
		{"Or true", "hasSelection || hasMultiCursor", ResolverContext{HasSelection: false, HasMultiCursor: true}, true},
		{"Or false", "hasSelection || hasMultiCursor", ResolverContext{HasSelection: false, HasMultiCursor: false}, false},
		{"Not true", "!readOnly", ResolverContext{ReadOnly: false}, true},
		{"Not false", "!readOnly", ResolverContext{ReadOnly: true}, false},
		{"Complex true", "(editorFocused && hasMultiCursor) || !readOnly", ResolverContext{EditorFocused: false, ReadOnly: false}, true},
		{"Complex false", "(editorFocused && hasMultiCursor) || !readOnly", ResolverContext{EditorFocused: false, ReadOnly: true}, false},
		{"Selection + Multi-Cursor composition", "hasSelection && !hasMultiCursor", ResolverContext{HasSelection: true, HasMultiCursor: false}, true},
		{"Selection + Multi-Cursor composition 2", "hasSelection && !hasMultiCursor", ResolverContext{HasSelection: true, HasMultiCursor: true}, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			b := []Binding{{Chords: []Chord{{Key: "a"}}, Command: "cmd", When: tt.when}}
			res, err := NewResolver(b)
			if err != nil {
				t.Fatalf("Failed to create resolver: %v", err)
			}
			_, result := res.Resolve(Chord{Key: "a"}, tt.ctx)
			if tt.expected {
				if result.Kind != ResultFound {
					t.Errorf("Expected ResultFound, got %v", result.Kind)
				}
			} else {
				if result.Kind != ResultNoMatch {
					t.Errorf("Expected ResultNoMatch, got %v", result.Kind)
				}
			}
		})
	}
}

func TestPendingDisplay(t *testing.T) {
	b := []Binding{{Chords: []Chord{{Ctrl: true, Key: "k"}, {Key: "a"}}, Command: "cmd"}}
	res, _ := NewResolver(b)
	if res.PendingDisplay() != "" {
		t.Errorf("Expected empty PendingDisplay, got %v", res.PendingDisplay())
	}

	res, _ = res.Resolve(Chord{Ctrl: true, Key: "k"}, ResolverContext{})
	expected := "Ctrl+K ..."
	if res.PendingDisplay() != expected {
		t.Errorf("Expected PendingDisplay %q, got %q", expected, res.PendingDisplay())
	}
}

func TestReset(t *testing.T) {
	b := []Binding{{Chords: []Chord{{Ctrl: true, Key: "k"}, {Key: "a"}}, Command: "cmd"}}
	res, _ := NewResolver(b)
	res, _ = res.Resolve(Chord{Ctrl: true, Key: "k"}, ResolverContext{})
	if !res.InChordMode() {
		t.Errorf("Expected InChordMode to be true")
	}

	res = res.Reset()
	if res.InChordMode() {
		t.Errorf("Expected InChordMode to be false after Reset()")
	}
}
