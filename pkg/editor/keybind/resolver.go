package keybind

import (
	"fmt"
	"strings"

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
	c.Key = kRaw.String()
	return c
}

type Binding struct {
	Chords  []Chord
	Command string
	When    string
}

type ResultKind int

const (
	ResultNoMatch ResultKind = iota
	ResultMoreChordsNeeded
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
	InCodeFence    bool
	ReadOnly       bool
}

type compiledBinding struct {
	chords  []Chord
	command string
	when    exprNode
	whenStr string
}

type Resolver struct {
	bindings       []compiledBinding
	pending        []Chord
	timeoutCommand string
}

func NewResolver(bindings []Binding) (Resolver, error) {
	var compiled []compiledBinding
	for i, b := range bindings {
		if len(b.Chords) == 0 {
			return Resolver{}, fmt.Errorf("binding %d has no chords", i)
		}
		expr, err := parseWhen(b.When)
		if err != nil {
			return Resolver{}, fmt.Errorf("parsing 'when' for binding %d: %w", i, err)
		}
		compiled = append(compiled, compiledBinding{
			chords:  b.Chords,
			command: b.Command,
			when:    expr,
			whenStr: b.When,
		})
	}

	for i, a := range compiled {
		for j := i + 1; j < len(compiled); j++ {
			b := compiled[j]
			if len(a.chords) == len(b.chords) && a.whenStr == b.whenStr {
				match := true
				for k := range a.chords {
					if a.chords[k] != b.chords[k] {
						match = false
						break
					}
				}
				if match {
					return Resolver{}, fmt.Errorf("duplicate bindings for identical chords and context: %s vs %s", a.command, b.command)
				}
			}
		}
	}

	return Resolver{bindings: compiled, pending: nil}, nil
}

func (r Resolver) Resolve(chord Chord, ctx ResolverContext) (Resolver, ResolutionResult) {
	newPending := append(append([]Chord(nil), r.pending...), chord)
	var exact []compiledBinding
	var longer []compiledBinding

	for _, b := range r.bindings {
		if len(b.chords) < len(newPending) {
			continue
		}
		match := true
		for i := range newPending {
			if b.chords[i] != newPending[i] {
				match = false
				break
			}
		}
		if !match {
			continue
		}
		if b.when != nil && !b.when.Eval(ctx) {
			// context predicate filtering (unfocused -> no-match)
			continue
		}

		if len(b.chords) == len(newPending) {
			exact = append(exact, b)
		} else {
			longer = append(longer, b)
		}
	}

	if len(exact) == 0 && len(longer) == 0 {
		return Resolver{bindings: r.bindings}, ResolutionResult{Kind: ResultNoMatch}
	}

	if len(exact) > 0 && len(longer) == 0 {
		// "Exact full match AND no longer candidate -> ResultFound"
		return Resolver{bindings: r.bindings}, ResolutionResult{Kind: ResultFound, Command: exact[0].command}
	}

	if len(exact) == 0 && len(longer) > 0 {
		// "All remaining have longer Chords -> ResultMoreChordsNeeded"
		return Resolver{bindings: r.bindings, pending: newPending}, ResolutionResult{Kind: ResultMoreChordsNeeded}
	}

	// Mixed
	return Resolver{
		bindings:       r.bindings,
		pending:        newPending,
		timeoutCommand: exact[0].command, // Shortest full match
	}, ResolutionResult{Kind: ResultMoreChordsNeeded}
}

func (r Resolver) ResolveTimeout() (Resolver, ResolutionResult) {
	if r.timeoutCommand != "" {
		return Resolver{bindings: r.bindings}, ResolutionResult{Kind: ResultFound, Command: r.timeoutCommand}
	}
	return Resolver{bindings: r.bindings}, ResolutionResult{Kind: ResultNoMatch}
}

func (r Resolver) Reset() Resolver {
	return Resolver{bindings: r.bindings, pending: nil, timeoutCommand: ""}
}

func (r Resolver) InChordMode() bool {
	return len(r.pending) > 0
}

func (r Resolver) PendingDisplay() string {
	if len(r.pending) == 0 {
		return ""
	}
	var parts []string
	for _, c := range r.pending {
		parts = append(parts, formatChord(c))
	}
	return strings.Join(parts, " ") + " ..."
}

func formatChord(c Chord) string {
	var p []string
	if c.Ctrl {
		p = append(p, "Ctrl")
	}
	if c.Alt {
		p = append(p, "Alt")
	}
	if c.Shift {
		p = append(p, "Shift")
	}
	if c.Cmd {
		p = append(p, "Cmd")
	}
	if len(c.Key) > 0 {
		p = append(p, strings.ToUpper(c.Key[:1])+c.Key[1:])
	}
	return strings.Join(p, "+")
}
