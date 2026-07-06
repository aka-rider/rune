//go:build fuzzing

package ai

import "errors"

// NewClient under the fuzzing build always refuses, regardless of
// environment: the session fuzzer must be deterministic and hermetic, and a
// fuzz input that submits a chat message would otherwise drive a real HTTP
// request from inside the fuzz worker (an unbounded wall-clock stall that
// the Go fuzz coordinator kills as "hung or terminated unexpectedly").
// Mirrors the fsnotify-watcher and footer-timer fuzz stubs: no network
// analogue exists in vfs.Mem, so the boundary is stubbed and the chat
// component's initErr path renders the refusal.
func NewClient() (Client, error) {
	return Client{}, errors.New("ai: disabled under the fuzzing build")
}
