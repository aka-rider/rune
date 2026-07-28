package keybind

import (
	"fmt"

	tea "charm.land/bubbletea/v2"
)

type Chord struct {
	Ctrl  bool
	Shift bool
	Alt   bool
	Cmd   bool
	Key   string
}

func ChordFromKeyMsg(msg tea.KeyPressMsg) Chord {
	k := msg.Key()
	c := Chord{
		Ctrl:  (k.Mod & tea.ModCtrl) != 0,
		Alt:   (k.Mod & tea.ModAlt) != 0,
		Shift: (k.Mod & tea.ModShift) != 0,
		Cmd:   (k.Mod & tea.ModSuper) != 0,
	}

	kRaw := k
	kRaw.Mod = 0
	kRaw.Text = ""
	key := kRaw.String()

	// Map Cyrillic characters to their US layout equivalents.
	// This allows Cmd+Z, Cmd+X, etc. to work on non-English layouts.
	if runes := []rune(key); len(runes) == 1 {
		key = string(cyrillicToUS(runes[0]))
	}
	c.Key = key
	return c
}

// cyrillicToUS maps a Cyrillic character to its US QWERTY layout equivalent.
// Returns the original character if no mapping exists.
func cyrillicToUS(r rune) rune {
	switch r {
	// Ukrainian layout → US QWERTY position mapping
	case 'я', 'Я':
		return 'z'
	case 'ч', 'Ч':
		return 'x'
	case 'ц', 'Ц':
		return 'c'
	case 'к', 'К':
		return 'v'
	case 'е', 'Е':
		return 'b'
	case 'н', 'Н':
		return 'n'
	case 'г', 'Г':
		return 'h'
	case 'ш', 'Ш':
		return 'm'
	case 'щ', 'Щ':
		return ','
	case 'з', 'З':
		return '.'
	case 'х', 'Х':
		return '/'
	case 'й', 'Й':
		return ';'
	case 'ў', 'Ў':
		return '\''
	case 'ё', 'Ё':
		return '"'
	case 'є', 'Є':
		return '\''
	case 'ї', 'Ї':
		return '\''
	default:
		return r
	}
}

type Binding struct {
	Chords  []Chord
	Command string
	When    string
}

type ResultKind int

const (
	ResultNoMatch ResultKind = iota
	ResultFound
)

type ResolutionResult struct {
	Kind    ResultKind
	Command string
}

type ResolverContext struct {
	EditorFocused  bool
	HasSelection   bool
	HasMultiCursor bool
	ReadOnly       bool
}

type compiledBinding struct {
	chord   Chord
	command string
	when    exprNode
	whenStr string
}

// Resolver is a stateless, config-only chord matcher: built once from a
// binding table (NewResolver) and consulted per keystroke via Resolve, which
// takes no receiver state and returns no updated Resolver — every keypress
// resolves independently of the last (§2.1's "no machine" verdict). This
// replaces a prior multi-chord "pending" state machine that was unreachable
// in production: keymap.CommandBindings (the only production binding
// producer) always emits single-chord bindings, so a chord sequence like
// Ctrl+K Ctrl+V never actually matched anything — NewResolver below rejects
// multi-chord bindings outright instead of silently accepting dead config.
//
// Reintroduction recipe, if chord sequences are ever wanted: give
// textedit.Model its own `pendingChords []Chord` (cleared on blur — the old
// resolver-owned pending survived blur/refocus by accident, which was itself
// a bug), and have Resolve accept/return that slice explicitly rather than
// hiding it in Resolver's receiver.
type Resolver struct {
	bindings []compiledBinding
}

// NewResolver compiles bindings into a Resolver. It errors on a malformed
// `when` expression, an exact duplicate (same chord + same when scope) across
// two bindings, or any binding with more than one chord — chord sequences
// are not supported (see the Resolver doc comment); rejecting them here keeps
// a config mistake honest instead of letting it silently never match.
func NewResolver(bindings []Binding) (Resolver, error) {
	var compiled []compiledBinding
	for i, b := range bindings {
		if len(b.Chords) == 0 {
			return Resolver{}, fmt.Errorf("binding %d has no chords", i)
		}
		if len(b.Chords) > 1 {
			return Resolver{}, fmt.Errorf("binding %d (%s): chord sequences are not supported", i, b.Command)
		}
		expr, err := parseWhen(b.When)
		if err != nil {
			return Resolver{}, fmt.Errorf("parsing 'when' for binding %d: %w", i, err)
		}
		compiled = append(compiled, compiledBinding{
			chord:   b.Chords[0],
			command: b.Command,
			when:    expr,
			whenStr: b.When,
		})
	}

	for i, a := range compiled {
		for j := i + 1; j < len(compiled); j++ {
			b := compiled[j]
			if a.whenStr == b.whenStr && a.chord == b.chord {
				return Resolver{}, fmt.Errorf("duplicate bindings for identical chords and context: %s vs %s", a.command, b.command)
			}
		}
	}

	return Resolver{bindings: compiled}, nil
}

// Resolve is a pure read-only lookup: chord × context → command. No pending
// state, no reassignment — call it fresh on every keypress.
func (r Resolver) Resolve(chord Chord, ctx ResolverContext) ResolutionResult {
	for _, b := range r.bindings {
		if b.chord != chord {
			continue
		}
		if b.when != nil && !b.when.Eval(ctx) {
			// context predicate filtering (unfocused -> no-match)
			continue
		}
		return ResolutionResult{Kind: ResultFound, Command: b.command}
	}
	return ResolutionResult{Kind: ResultNoMatch}
}
