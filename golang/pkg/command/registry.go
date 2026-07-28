package command

import (
	"errors"
	"sort"
	"strings"
)

type Builder struct {
	commands map[string]Command
}

type Registry struct {
	commands map[string]Command
}

func NewBuilder() Builder {
	return Builder{
		commands: make(map[string]Command),
	}
}

func (b Builder) Register(cmd Command) (Builder, error) {
	if b.commands == nil {
		b.commands = make(map[string]Command)
	}

	// Copy for aliasing safety
	newCmds := make(map[string]Command, len(b.commands)+1)
	for k, v := range b.commands {
		newCmds[k] = v
	}

	if _, exists := newCmds[cmd.Name]; exists {
		return Builder{commands: newCmds}, errors.New("command already registered: " + cmd.Name)
	}

	newCmds[cmd.Name] = cmd

	return Builder{commands: newCmds}, nil
}

func (b Builder) Build() Registry {
	// Copy to ensure immutability
	cmds := make(map[string]Command, len(b.commands))
	for k, v := range b.commands {
		cmds[k] = v
	}
	return Registry{commands: cmds}
}

func (r Registry) Get(name string) (Command, bool) {
	if r.commands == nil {
		return Command{}, false
	}
	cmd, ok := r.commands[name]
	return cmd, ok
}

func (r Registry) Execute(name string, ctx CommandContext) Result {
	cmd, ok := r.Get(name)
	if !ok {
		return Result{Err: errors.New("command not found: " + name)}
	}
	if cmd.Execute == nil {
		return Result{Err: errors.New("command has no Execute func: " + name)}
	}
	return cmd.Execute(ctx)
}

func (r Registry) Search(query string) []Command {
	if r.commands == nil {
		return nil
	}

	q := strings.ToLower(query)
	var matches []Command

	for _, cmd := range r.commands {
		if strings.Contains(strings.ToLower(cmd.Name), q) ||
			strings.Contains(strings.ToLower(cmd.Title), q) {
			matches = append(matches, cmd)
		}
	}

	sort.SliceStable(matches, func(i, j int) bool {
		return matches[i].Name < matches[j].Name
	})

	return matches
}

func (r Registry) All() []Command {
	if r.commands == nil {
		return nil
	}
	var all []Command
	for _, cmd := range r.commands {
		all = append(all, cmd)
	}
	sort.SliceStable(all, func(i, j int) bool {
		return all[i].Name < all[j].Name
	})
	return all
}
