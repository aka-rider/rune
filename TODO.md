# Ledger

- The `merge_begin_over_active` integration test flakes under parallel load (roughly 1 in 15 runs of `cargo nextest run -p rune-tui -E 'test(merge)'`): the assertion "the torn-down first session's row must not be left active" reads 1 where 0 is expected. Reproduced at commit `a952ee94` before any project-search preview work, so it is a pre-existing race — either the second `merge::begin` really can leave the first session's `merges` row active, or the test's "every enqueued op has been drained" claim has a gap. Needs a root-cause pass; violates the no-flakiness testing rule.
