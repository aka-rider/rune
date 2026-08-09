---
description: Run make test-fuzz; on a catch, a forced-plan-mode subagent finds the root cause and reports on screen.
---

1. Run `make test-fuzz RC=8192` from the repo root **in the background** (`run_in_background: true`) —
   `RC` is the number of randomized sessions (PROPTEST_CASES) and a run at this size takes a long
   while, so any foreground timeout is insufficient. Add `RS=<seed>` to re-run a pinned seed.
   **Do not poll it.** A background run re-invokes you automatically when it exits; just wait for that
   completion notification, then read the full output. Never busy-check its output (no per-second
   polling). If you must check liveness, do so at most every few minutes.
2. Passes → report green. Done.
3. Fails → Call the `Agent` tool with `subagent_type: "rune-fuzz-investigator"`, `model: "sonnet"` with the failing target and its `invariant <ID>: <message>` line. Store its findings in <TODO-brief.md>. Do not edit.
