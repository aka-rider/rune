---
description: Run make test-fuzz; on a catch, a forced-plan-mode subagent finds the root cause and reports on screen.
---

1. Run `make test-fuzz T=25m` **in the background** (`run_in_background: true`) — time T is per target
   (is a few hours), so any foreground timeout is insufficient.
   **Do not poll it.** A background run re-invokes you automatically when it exits; just wait for that
   completion notification, then read the full output. Never busy-check its output (no per-second
   polling). If you must check liveness, do so at most every few minutes.
   (env: `export GOPATH=/Users/xiii/Developer/go GOCACHE=/Users/xiii/Developer/go/build-cache GOFLAGS=-buildvcs=false TMPDIR=/Users/xiii/Developer/go/rune-tmp`).
2. Passes → report green. Done.
3. Fails → Call the `Agent` tool with `subagent_type: "rune-fuzz-investigator"`, `model: "sonnet"` with the failing target and its `invariant <ID>: <message>` line. Store its findings in <TODO-brief.md>. Do not edit.
