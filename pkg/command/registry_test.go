package command

import (
	"errors"
	"sync"
	"testing"
)

func TestBuilder_Register(t *testing.T) {
	b := NewBuilder()

	cmd1 := Command{Name: "test.cmd1", Title: "Test Cmd 1"}
	b2, err := b.Register(cmd1)
	if err != nil {
		t.Fatalf("unexpected error on first register: %v", err)
	}

	if _, ok := b.commands["test.cmd1"]; ok {
		t.Errorf("original builder modified by register")
	}
	if _, ok := b2.commands["test.cmd1"]; !ok {
		t.Errorf("new builder missing registered command")
	}

	// Test duplicate
	_, err = b2.Register(cmd1)
	if err == nil {
		t.Errorf("expected error on duplicate register, got nil")
	}
}

func TestBuilder_AliasingSafety(t *testing.T) {
	b1 := NewBuilder()
	b2, _ := b1.Register(Command{Name: "cmd1"})

	// b3 branches from b2
	_, _ = b2.Register(Command{Name: "cmd2", Title: "Original"})

	// b4 branches from b2 (should not see cmd2)
	b4, err := b2.Register(Command{Name: "cmd3"})

	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if _, ok := b4.commands["cmd2"]; ok {
		t.Errorf("b4 sees cmd2 from b3 branch - aliasing violation")
	}
}

func TestRegistry_Immutability(t *testing.T) {
	b := NewBuilder()
	b, _ = b.Register(Command{Name: "cmd1"})

	reg := b.Build()

	// Modify builder after build
	_, _ = b.Register(Command{Name: "cmd2"})

	if _, ok := reg.Get("cmd2"); ok {
		t.Errorf("registry mutated after build")
	}
}

func TestRegistry_Concurrency(t *testing.T) {
	b := NewBuilder()
	cmds := []string{"cmd1", "cmd2", "cmd3"}
	for _, name := range cmds {
		b, _ = b.Register(Command{Name: name})
	}
	reg := b.Build()

	var wg sync.WaitGroup
	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			cmdName := cmds[idx%len(cmds)]
			cmd, ok := reg.Get(cmdName)
			if !ok {
				t.Errorf("concurrent get failed to find %q", cmdName)
			}
			if cmd.Name != cmdName {
				t.Errorf("concurrent get found wrong command: %q", cmd.Name)
			}
		}(i)
	}
	wg.Wait()
}

func TestRegistry_Execute(t *testing.T) {
	b := NewBuilder()

	executed := false
	execErr := errors.New("exec fail")

	cmd := Command{
		Name: "test.exec",
		Execute: func(ctx CommandContext) Result {
			executed = true
			if ctx.FilePath != "test.txt" {
				return Result{Err: errors.New("wrong filepath")}
			}
			return Result{Err: execErr}
		},
	}

	b, _ = b.Register(cmd)
	reg := b.Build()

	ctx := CommandContext{FilePath: "test.txt"}
	res := reg.Execute("test.exec", ctx)

	if !executed {
		t.Errorf("execute func not called")
	}
	if res.Err != execErr {
		t.Errorf("expected err %v, got %v", execErr, res.Err)
	}

	// Missing cmd Test
	missingRes := reg.Execute("missing", ctx)
	if missingRes.Err == nil {
		t.Errorf("expected error executing missing command, got nil")
	}
}

func TestRegistry_Search(t *testing.T) {
	b := NewBuilder()

	setupCmds := []Command{
		{Name: "file.save", Title: "Save File"},
		{Name: "file.save_as", Title: "Save File As"},
		{Name: "editor.quit", Title: "Quit Editor"},
		{Name: "search.find", Title: "Find text"},
		{Name: "apple.save", Title: "Save Apple"}, // To test sorting ties, apple should be first
	}

	for _, c := range setupCmds {
		b, _ = b.Register(c)
	}
	reg := b.Build()

	tests := []struct {
		query    string
		expected []string // names
	}{
		{"save", []string{"apple.save", "file.save", "file.save_as"}},
		{"EDIT", []string{"editor.quit"}}, // case insensitive on Name/Title
		{"find TEXT", []string{"search.find"}},
		{"missing", []string{}},
	}

	for _, tc := range tests {
		res := reg.Search(tc.query)
		if len(res) != len(tc.expected) {
			t.Errorf("search %q: expected %d results, got %d", tc.query, len(tc.expected), len(res))
			continue
		}
		for i, v := range res {
			if v.Name != tc.expected[i] {
				t.Errorf("search %q: at index %d expected %q, got %q", tc.query, i, tc.expected[i], v.Name)
			}
		}
	}
}

func TestRegistry_All(t *testing.T) {
	b := NewBuilder()
	cmds := []Command{
		{Name: "z"},
		{Name: "a"},
		{Name: "c"},
	}
	for _, c := range cmds {
		b, _ = b.Register(c)
	}
	reg := b.Build()

	all := reg.All()
	if len(all) != 3 {
		t.Fatalf("expected 3 cmds, got %d", len(all))
	}
	if all[0].Name != "a" || all[1].Name != "c" || all[2].Name != "z" {
		t.Errorf("All() not sorted alphabetically: %v", all)
	}
}
