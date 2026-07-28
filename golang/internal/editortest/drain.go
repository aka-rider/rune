package editortest

// Cmd-draining chokepoints (Phase 4 of the QA-rehaul plan): every test that
// needs to settle an async Bubble Tea round trip — keypress → footer response
// msg → materializeCmd → FileSavedMsg — deterministically, with no
// time.Sleep and no reliance on runtime scheduling, funnels through ExecCmds/
// Drain/DrainUntil instead of a per-file copy of the same loop. Drain's
// `after` hooks are the seam where per-step invariant checking plugs in
// (workspace's settle passes session.Check over m.FuzzInspect()).

import (
	"reflect"

	tea "charm.land/bubbletea/v2"
)

// cmdSliceType lets asCmdSlice recognize any message whose underlying type is
// []tea.Cmd — tea.BatchMsg directly, and bubbletea's UNEXPORTED sequenceMsg
// (tea.Sequence's container) via reflection, which a plain type assertion
// cannot name.
var cmdSliceType = reflect.TypeOf([]tea.Cmd(nil))

func asCmdSlice(msg tea.Msg) ([]tea.Cmd, bool) {
	if msg == nil {
		return nil, false
	}
	if batch, ok := msg.(tea.BatchMsg); ok {
		return []tea.Cmd(batch), true
	}
	rv := reflect.ValueOf(msg)
	if rv.IsValid() && rv.Type().ConvertibleTo(cmdSliceType) {
		return rv.Convert(cmdSliceType).Interface().([]tea.Cmd), true
	}
	return nil, false
}

// ExecCmds executes cmd synchronously and collects every resulting message.
// Handles nil cmds and expands batch/sequence containers recursively (for
// test-settling purposes the sequential-vs-concurrent distinction is moot:
// everything runs synchronously in submission order).
func ExecCmds(cmd tea.Cmd) []tea.Msg {
	if cmd == nil {
		return nil
	}
	msg := cmd()
	if msg == nil {
		return nil
	}
	if cmds, ok := asCmdSlice(msg); ok {
		var msgs []tea.Msg
		for _, c := range cmds {
			msgs = append(msgs, ExecCmds(c)...)
		}
		return msgs
	}
	return []tea.Msg{msg}
}

// Drain delivers cmd's messages (and any message a resulting Cmd yields,
// recursively, breadth-first) through update, fully settling an async round
// trip within a single deterministic test step. Every `after` hook runs with
// the post-update model after EACH delivered message — the plug-in point for
// per-step invariant checking.
//
// update is the model's Update method value (e.g. workspace.Model.Update):
// for a value-receiver method, Model.Update has exactly the required
// func(Model, tea.Msg) (Model, tea.Cmd) shape.
func Drain[M any](m M, cmd tea.Cmd, update func(M, tea.Msg) (M, tea.Cmd), after ...func(M)) M {
	pending := ExecCmds(cmd)
	for len(pending) > 0 {
		msg := pending[0]
		pending = pending[1:]
		var next tea.Cmd
		m, next = update(m, msg)
		for _, f := range after {
			f(m)
		}
		pending = append(pending, ExecCmds(next)...)
	}
	return m
}

// DrainUntil is Drain but stops as soon as stop reports true — checked once
// on entry with a nil msg (the sought state may already hold), then after
// every delivered message BEFORE executing the Cmd that message's update
// returned. That ordering is load-bearing: the awaited result's own follow-up
// Cmd may be a REAL-TIME timer (e.g. a footer status message's auto-dismiss)
// that ExecCmds would run synchronously — blocking on time.Sleep and then
// clearing the very state the caller is waiting to observe.
func DrainUntil[M any](m M, cmd tea.Cmd, update func(M, tea.Msg) (M, tea.Cmd), stop func(m M, lastMsg tea.Msg) bool) M {
	if stop(m, nil) {
		return m
	}
	pending := ExecCmds(cmd)
	for len(pending) > 0 {
		msg := pending[0]
		pending = pending[1:]
		var next tea.Cmd
		m, next = update(m, msg)
		if stop(m, msg) {
			return m
		}
		pending = append(pending, ExecCmds(next)...)
	}
	return m
}
