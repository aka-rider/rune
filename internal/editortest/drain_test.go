package editortest

import (
	"testing"

	tea "charm.land/bubbletea/v2"
)

// counter is a minimal model: each int msg is appended to seen; a msg with
// value >0 returns a Cmd producing value-1, so a chain of N follow-up hops
// settles deterministically.
type counter struct{ seen []int }

func (c counter) update(msg tea.Msg) (counter, tea.Cmd) {
	n, ok := msg.(int)
	if !ok {
		return c, nil
	}
	c.seen = append(c.seen, n)
	if n > 0 {
		next := n - 1
		return c, func() tea.Msg { return next }
	}
	return c, nil
}

func TestExecCmds_ExpandsBatchAndSequence(t *testing.T) {
	one := func() tea.Msg { return 1 }
	two := func() tea.Msg { return 2 }

	batch := ExecCmds(tea.Batch(one, two))
	if len(batch) != 2 || batch[0] != 1 || batch[1] != 2 {
		t.Fatalf("batch: got %v, want [1 2]", batch)
	}

	// tea.Sequence yields bubbletea's unexported sequenceMsg ([]tea.Cmd
	// underneath) — only the reflect path can expand it.
	seq := ExecCmds(tea.Sequence(one, two))
	if len(seq) != 2 || seq[0] != 1 || seq[1] != 2 {
		t.Fatalf("sequence: got %v, want [1 2]", seq)
	}

	if got := ExecCmds(nil); got != nil {
		t.Fatalf("nil cmd: got %v, want nil", got)
	}
}

func TestDrain_SettlesFollowUpCmdsAndRunsAfterHooks(t *testing.T) {
	var hooks int
	m := Drain(counter{}, func() tea.Msg { return 3 }, counter.update, func(counter) { hooks++ })
	want := []int{3, 2, 1, 0}
	if len(m.seen) != len(want) {
		t.Fatalf("seen %v, want %v", m.seen, want)
	}
	for i := range want {
		if m.seen[i] != want[i] {
			t.Fatalf("seen %v, want %v", m.seen, want)
		}
	}
	if hooks != len(want) {
		t.Fatalf("after hook ran %d times, want %d (once per delivered message)", hooks, len(want))
	}
}

func TestDrainUntil_StopsBeforeExecutingNextCmd(t *testing.T) {
	// Stop when 2 lands: the Cmd producing 1 must never execute.
	m := DrainUntil(counter{}, func() tea.Msg { return 3 }, counter.update,
		func(m counter, _ tea.Msg) bool {
			return len(m.seen) > 0 && m.seen[len(m.seen)-1] == 2
		})
	if len(m.seen) != 2 || m.seen[0] != 3 || m.seen[1] != 2 {
		t.Fatalf("seen %v, want [3 2]", m.seen)
	}
}
