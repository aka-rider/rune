CARGO ?= cargo
# Randomized sessions per `make test-fuzz` (PROPTEST_CASES). Defaulted here AND
# in the root Makefile: an EMPTY value is not "unset" — proptest warns to stderr
# and silently falls back to 256 (config.rs parse_or_warn).
RC ?= 512
# Optional PROPTEST_RNG_SEED for a pinned re-run. Empty = fresh OS entropy.
RS ?=

.PHONY: build test lint fmt bench perf-guard test-fuzz

build:
	$(CARGO) build --workspace

test:
	$(CARGO) test --workspace
	$(CARGO) test -p rune-md --features strict-invariants

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt:
	$(CARGO) fmt --all --check

bench:
	$(CARGO) bench -p rune-md --bench parse_bench

# `--test perf_guard` + `--exact` scope this to ONE test in ONE binary. Without
# them, every other rune-md test binary prints `test result: ok. 0 passed ...
# filtered out` and exits 0, so the target stays green even if the test is
# renamed or deleted.
perf-guard:
	$(CARGO) test -p rune-md --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 full_pipeline_5k_under_100ms

# `-p rune-fuzz` (NOT --workspace) is load-bearing: under --workspace, cargo
# feature-unifies rune-md's dev-dependency on itself and compiles rune-md with
# `strict-invariants`, whose known-open comrak sourcepos panics
# (crates/rune-md/TODO.md) would drown the session fuzzer in non-bugs.
# `--test human_session` + `--exact` for the same reason as perf-guard.
# Debug profile on purpose: keeps the buffer/undo/render debug_asserts armed.
test-fuzz:
	PROPTEST_CASES=$(RC) PROPTEST_RNG_SEED=$(RS) \
	    $(CARGO) test -p rune-fuzz --test human_session -- \
	    --ignored --exact --test-threads=1 human_session
