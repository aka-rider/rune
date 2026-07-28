package textedit

import "rune/pkg/command"

// cmdSpec is one registerable command, declarative enough that a family's
// entire Register block collapses to a slice literal + registerAll. category/
// title are the only fields command.Command carries that cmdSpec's callers
// occasionally need (clipboard.*); every other family leaves them zero.
type cmdSpec struct {
	name     string
	when     string
	exec     command.CommandFn
	category string
	title    string
}

// registerAll registers every spec in order, stopping at the first error —
// the single loop behind what used to be seven hand-unrolled
// builder.Register/err-check blocks, one per command family.
func registerAll(b command.Builder, specs []cmdSpec) (command.Builder, error) {
	var err error
	for _, s := range specs {
		b, err = b.Register(command.Command{
			Name:     s.name,
			Category: s.category,
			Title:    s.title,
			When:     s.when,
			Execute:  s.exec,
		})
		if err != nil {
			return b, err
		}
	}
	return b, nil
}

// RegisterCommands registers every textedit command family into builder.
// app.go's startup binding<->command verification loop is the real check
// that this list stays complete.
func RegisterCommands(builder command.Builder) (command.Builder, error) {
	var err error
	for _, specs := range [][]cmdSpec{
		navSpecs,
		editSpecs,
		indentSpecs,
		multiLineSpecs,
		multiSpecs,
		clipboardSpecs,
		findSpecs,
	} {
		builder, err = registerAll(builder, specs)
		if err != nil {
			return builder, err
		}
	}
	return builder, nil
}
