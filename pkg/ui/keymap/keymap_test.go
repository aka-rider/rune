package keymap

import (
	"fmt"
	"reflect"
	"testing"

	"rune/pkg/editor/keybind"
)

// globalKeys lists the Binding field names whose keys are handled as
// page-level globals (Priority 3 in workspace.Update) rather than
// wired to commands via CommandBindings.
var globalKeys = []string{
	"TabSwitch",
	"PinTab",
	"FocusExplorer",
	"FocusEditor",
	"FocusChat",
	"CreateNewFile",
	"CloseFile",
	"ZenMode",
	"ConfirmExitC",
	"ConfirmExitD",
	"Help",
	"VoiceDictation",
	"SaveFile", // workspace-global Cmd+S (D4/D12: not a registered command; handled directly)
}

// editorShortcutKeys lists Binding field names handled as hardcoded
// shortcuts inside the editor's Update() method (not via the resolver).
var editorShortcutKeys = []string{
	"PrimaryAction", // Enter → newline
	"Cancel",        // Escape → cancel/close modal
	"Undo",          // Cmd+Z → undo
	"Redo",          // Cmd+Shift+Z → redo
}

// chordToKeyString reconstructs the key string that parseChord() would
// have produced from a Chord, so we can match against AllPhysicalKeys().
func chordToKeyString(c keybind.Chord) string {
	parts := make([]string, 0, 4)
	if c.Ctrl {
		parts = append(parts, "ctrl")
	}
	if c.Alt {
		parts = append(parts, "alt")
	}
	if c.Shift {
		parts = append(parts, "shift")
	}
	if c.Cmd {
		parts = append(parts, "cmd")
	}
	if c.Key != "" {
		parts = append(parts, c.Key)
	}
	if len(parts) == 0 {
		return ""
	}
	return fmt.Sprintf("%s", parts[0]) // Simplified: for multi-part chords we need join
}

// keyFromChord builds the key string that parseChord would produce.
func keyFromChord(c keybind.Chord) string {
	if !c.Ctrl && !c.Alt && !c.Shift && !c.Cmd {
		return c.Key
	}
	var parts []string
	if c.Ctrl {
		parts = append(parts, "ctrl")
	}
	if c.Alt {
		parts = append(parts, "alt")
	}
	if c.Shift {
		parts = append(parts, "shift")
	}
	if c.Cmd {
		parts = append(parts, "super")
	}
	parts = append(parts, c.Key)
	result := parts[0]
	for i := 1; i < len(parts); i++ {
		result = result + "+" + parts[i]
	}
	return result
}

// TestBindingsFullyWired ensures every key.Binding field on Bindings is
// either wired to a command in CommandBindings(), handled as a global
// key in the workspace page, or handled as a hardcoded shortcut in the
// editor. This prevents the same class of bug from recurring
// (bindings defined but never mapped to any handler).
func TestBindingsFullyWired(t *testing.T) {
	keys := Default()

	// Collect all physical keys from AllPhysicalKeys().
	allKeys := make(map[string]bool)
	for _, k := range keys.AllPhysicalKeys() {
		allKeys[k] = true
	}

	// Collect all physical keys from CommandBindings.
	cmdBindings, err := keys.CommandBindings()
	if err != nil {
		t.Fatalf("CommandBindings returned error: %v", err)
	}
	wiredKeys := make(map[string]bool)
	for _, b := range cmdBindings {
		for _, chord := range b.Chords {
			k := keyFromChord(chord)
			if k != "" {
				wiredKeys[k] = true
			}
		}
	}

	// Collect all physical keys from global key bindings.
	v := reflect.ValueOf(keys)
	typ := v.Type()
	for i := 0; i < v.NumField(); i++ {
		fieldName := typ.Field(i).Name
		for _, gk := range globalKeys {
			if fieldName == gk {
				keysMethod := v.Field(i).MethodByName("Keys")
				if !keysMethod.IsValid() {
					continue
				}
				result := keysMethod.Call(nil)
				if len(result) == 0 {
					continue
				}
				keySlice, ok := result[0].Interface().([]string)
				if !ok {
					continue
				}
				for _, k := range keySlice {
					wiredKeys[k] = true
				}
			}
		}
	}

	// Collect all physical keys from editor shortcut bindings.
	for i := 0; i < v.NumField(); i++ {
		fieldName := typ.Field(i).Name
		for _, ek := range editorShortcutKeys {
			if fieldName == ek {
				keysMethod := v.Field(i).MethodByName("Keys")
				if !keysMethod.IsValid() {
					continue
				}
				result := keysMethod.Call(nil)
				if len(result) == 0 {
					continue
				}
				keySlice, ok := result[0].Interface().([]string)
				if !ok {
					continue
				}
				for _, k := range keySlice {
					wiredKeys[k] = true
				}
			}
		}
	}

	// Assert: every binding's keys must be wired (command, global, or shortcut).
	for i := 0; i < v.NumField(); i++ {
		fieldName := typ.Field(i).Name
		keysMethod := v.Field(i).MethodByName("Keys")
		if !keysMethod.IsValid() {
			continue
		}
		result := keysMethod.Call(nil)
		if len(result) == 0 {
			continue
		}
		keySlice, ok := result[0].Interface().([]string)
		if !ok {
			continue
		}
		for _, k := range keySlice {
			if !wiredKeys[k] {
				t.Errorf("key %q on binding %q is not wired to any command, global handler, or editor shortcut", k, fieldName)
			}
		}
	}

	// Sanity: AllPhysicalKeys() must cover every key that appears in
	// CommandBindings or the global/shortcut lists.
	for k := range allKeys {
		if !wiredKeys[k] {
			t.Errorf("key %q is in AllPhysicalKeys() but not in any handler list", k)
		}
	}
}

// TestAllHelpCoversBindings ensures the reflection-based help enumeration
// yields an entry for every documented binding (non-empty key) and includes
// the renamed Help binding. This is the single source for the in-app help doc.
func TestAllHelpCoversBindings(t *testing.T) {
	entries := Default().AllHelp()
	if len(entries) == 0 {
		t.Fatal("AllHelp returned no entries")
	}
	for _, e := range entries {
		if e.Key == "" {
			t.Errorf("help entry has empty key: %+v", e)
		}
	}
	found := false
	for _, e := range entries {
		if e.Key == "F1" && e.Desc == "help" {
			found = true
			break
		}
	}
	if !found {
		t.Error("AllHelp missing the Help (F1) entry")
	}
}

func TestNoKeybindingCollisions(t *testing.T) {
	keys := Default()
	physicalKeys := keys.AllPhysicalKeys()

	seen := make(map[string]bool)
	for _, k := range physicalKeys {
		if seen[k] {
			t.Errorf("duplicate physical key string detected: %q", k)
		}
		seen[k] = true
	}

	err := keys.ValidateNoPhysicalKeyCollisions()
	if err != nil {
		t.Errorf("ValidateNoPhysicalKeyCollisions returned error: %v", err)
	}
}

// TestKeybindingCollisionsWithFieldNames provides detailed collision reporting
// showing which Bindings fields conflict.
func TestKeybindingCollisionsWithFieldNames(t *testing.T) {
	keys := Default()
	v := reflect.ValueOf(keys)
	typ := v.Type()

	// Map: physical key string → list of field names that use it
	keyToFields := make(map[string][]string)

	for i := 0; i < v.NumField(); i++ {
		field := v.Field(i)
		fieldName := typ.Field(i).Name

		// Each field is a key.Binding; call Keys() to get its physical keys
		keysMethod := field.MethodByName("Keys")
		if !keysMethod.IsValid() {
			continue
		}
		result := keysMethod.Call(nil)
		if len(result) == 0 {
			continue
		}
		keySlice, ok := result[0].Interface().([]string)
		if !ok {
			continue
		}
		for _, k := range keySlice {
			keyToFields[k] = append(keyToFields[k], fieldName)
		}
	}

	for keyStr, fields := range keyToFields {
		if len(fields) > 1 {
			t.Errorf("physical key %q appears in multiple bindings: %v", keyStr, fields)
		}
	}
}

// TestAllBindingsHaveKeys ensures no binding field is left empty.
func TestAllBindingsHaveKeys(t *testing.T) {
	keys := Default()
	v := reflect.ValueOf(keys)
	typ := v.Type()

	for i := 0; i < v.NumField(); i++ {
		field := v.Field(i)
		fieldName := typ.Field(i).Name

		keysMethod := field.MethodByName("Keys")
		if !keysMethod.IsValid() {
			continue
		}
		result := keysMethod.Call(nil)
		if len(result) == 0 {
			continue
		}
		keySlice, ok := result[0].Interface().([]string)
		if !ok {
			continue
		}
		if len(keySlice) == 0 {
			t.Errorf("binding %q has no physical keys assigned", fieldName)
		}
	}
}

// TestShiftNavigationResolvesToSelectCommands verifies that shift-prefixed
// navigation keys resolve to select.* commands (not cursor.* commands).
// This is the regression test for the shift-selection bug.
func TestShiftNavigationResolvesToSelectCommands(t *testing.T) {
	keys := Default()
	mappings, err := keys.CommandBindings()
	if err != nil {
		t.Fatalf("CommandBindings error: %v", err)
	}

	resolver, err := keybind.NewResolver(mappings)
	if err != nil {
		t.Fatalf("NewResolver error: %v", err)
	}

	ctx := keybind.ResolverContext{EditorFocused: true}

	tests := []struct {
		chord    keybind.Chord
		expected string
	}{
		// Non-shifted navigation -> cursor commands
		{keybind.Chord{Key: "up"}, "cursor.line-up"},
		{keybind.Chord{Key: "down"}, "cursor.line-down"},
		{keybind.Chord{Key: "left"}, "cursor.character-left"},
		{keybind.Chord{Key: "right"}, "cursor.character-right"},
		{keybind.Chord{Key: "home"}, "cursor.line-start"},
		{keybind.Chord{Key: "end"}, "cursor.line-end"},
		{keybind.Chord{Key: "pgup"}, "cursor.page-up"},
		{keybind.Chord{Key: "pgdown"}, "cursor.page-down"},

		// Shifted navigation -> select commands
		{keybind.Chord{Shift: true, Key: "up"}, "select.line-up"},
		{keybind.Chord{Shift: true, Key: "down"}, "select.line-down"},
		{keybind.Chord{Shift: true, Key: "left"}, "select.character-left"},
		{keybind.Chord{Shift: true, Key: "right"}, "select.character-right"},
		{keybind.Chord{Shift: true, Key: "home"}, "select.line-start"},
		{keybind.Chord{Shift: true, Key: "end"}, "select.line-end"},
		{keybind.Chord{Shift: true, Key: "pgup"}, "select.page-up"},
		{keybind.Chord{Shift: true, Key: "pgdown"}, "select.page-down"},
		{keybind.Chord{Alt: true, Key: "left"}, "cursor.word-left"},
		{keybind.Chord{Alt: true, Key: "right"}, "cursor.word-right"},
		{keybind.Chord{Alt: true, Shift: true, Key: "left"}, "select.word-left"},
		{keybind.Chord{Alt: true, Shift: true, Key: "right"}, "select.word-right"},

		// Cmd+A -> select all
		{keybind.Chord{Cmd: true, Key: "a"}, "select.all"},
		// Clipboard
		{keybind.Chord{Cmd: true, Key: "c"}, "clipboard.copy"},
		{keybind.Chord{Cmd: true, Key: "x"}, "clipboard.cut"},
		{keybind.Chord{Cmd: true, Key: "v"}, "clipboard.paste"},
	}

	for _, tc := range tests {
		name := "shift+" + tc.chord.Key
		if !tc.chord.Shift {
			name = tc.chord.Key
		}
		t.Run(name, func(t *testing.T) {
			r, res := resolver.Resolve(tc.chord, ctx)
			if res.Kind != keybind.ResultFound {
				t.Fatalf("expected ResultFound, got %v (resolver=%+v)", res.Kind, r)
			}
			if res.Command != tc.expected {
				t.Fatalf("expected command %q, got %q", tc.expected, res.Command)
			}
		})
	}
}

// TestShiftVsCursorDisjoint ensures shift-prefixed bindings map to select.*
// commands while non-shifted bindings map to cursor.* commands. No overlap
// for navigation commands (select.all via Cmd+A is an exception).
func TestShiftVsCursorDisjoint(t *testing.T) {
	keys := Default()
	mappings, err := keys.CommandBindings()
	if err != nil {
		t.Fatalf("CommandBindings error: %v", err)
	}

	for _, m := range mappings {
		for _, chord := range m.Chords {
			if chord.Shift {
				if len(m.Command) >= 7 && m.Command[:7] == "cursor." {
					t.Errorf("shifted chord maps to cursor command: %s", m.Command)
				}
			} else {
				// select.all is intentionally non-shifted (Cmd+A)
				if m.Command == "select.all" {
					continue
				}
				if len(m.Command) >= 7 && m.Command[:7] == "select." {
					t.Errorf("non-shifted chord maps to select command: %s", m.Command)
				}
			}
		}
	}
}
