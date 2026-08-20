CARGO ?= cargo
# Randomized sessions per `make test-fuzz` (PROPTEST_CASES). An EMPTY value is
# not "unset" — proptest warns to stderr and silently falls back to 256
# (config.rs parse_or_warn) — so this default has to be spelled out.
RC ?= 512
# Optional PROPTEST_RNG_SEED for a pinned re-run. Empty = fresh OS entropy.
RS ?=

.PHONY: build test lint fmt bench perf-guard test-fuzz test-grammars dist-mac

SDK_CACHE := $(HOME)/.cache/rune/MacOSX14.5.sdk

build:
	$(CARGO) build --workspace

test:
	$(CARGO) test --workspace
	# Cargo feature unification already arms strict-invariants on rune-md for the
	# whole --workspace run above (rune-fuzz requests it), but this isolated
	# invocation stays so the suite still proves out if that unification ever goes away.
	$(CARGO) test -p rune-md --features strict-invariants

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) clippy --workspace --lib -- -D warnings

fmt:
	$(CARGO) fmt --all --check

bench:
	$(CARGO) bench -p rune-md --bench parse_bench

# `--test perf_guard` + `--exact` scope each invocation to ONE test in ONE
# binary. Without them, every other test binary prints `test result: ok. 0
# passed ... filtered out` and exits 0, so the target stays green even if the
# test is renamed or deleted — this is why the WP16 keystroke-latency guard
# below needs its OWN invocation rather than widening the `--exact` filter on
# this one, which by definition can never pick up a second test name.
perf-guard:
	$(CARGO) test -p rune-md --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 full_pipeline_5k_under_100ms
	$(CARGO) test -p rune-md --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 full_pipeline_cost_scales_linearly_not_quadratically_with_document_size
	$(CARGO) test -p rune-tui --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 keystroke_view_cost_under_budget_on_a_5k_line_code_document
	$(CARGO) test -p rune-tui --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 render_frame_cost_under_budget_on_a_5k_line_code_document
	$(CARGO) test -p rune-tui --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 render_frame_cost_under_budget_on_a_many_fence_markdown_document
	$(CARGO) test -p rune-tui --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 render_frame_cost_under_budget_with_the_caret_on_an_unmatched_bracket
	$(CARGO) test -p rune-tui --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 bootstrap_first_draw_stays_bounded_on_a_large_document

# `--test human_session` + `--exact` for the same reason as perf-guard.
# Debug profile on purpose: keeps the buffer/undo/render debug_asserts armed.
test-fuzz:
	PROPTEST_CASES=$(RC) PROPTEST_RNG_SEED=$(RS) \
	    $(CARGO) test -p rune-fuzz --test human_session -- \
	    --ignored --exact --test-threads=1 human_session

# The heavy per-grammar property test: every one of the 22 tree-sitter
# grammars against many arbitrary sources, not just the handful the
# non-ignored smoke test runs on every `make test`.
test-grammars:
	$(CARGO) test -p rune-ts --test grammar_props -- --ignored --test-threads=1

dist-mac:
	command -v nix >/dev/null || { echo "dist-mac needs nix on PATH (NixOS/macOS with nix installed)"; exit 1; }
	[ -d $(SDK_CACHE) ] || { \
	    mkdir -p $(HOME)/.cache/rune; \
	    curl -fL https://github.com/joseluisq/macosx-sdks/releases/download/14.5/MacOSX14.5.sdk.tar.xz \
	        | tar -xJ -C $(HOME)/.cache/rune; \
	}
	nix shell nixpkgs#rustup nixpkgs#gcc nixpkgs#zig nixpkgs#cargo-zigbuild -c \
	    sh -c 'rustup target add aarch64-apple-darwin && \
	        SDKROOT=$(SDK_CACHE) cargo zigbuild --target aarch64-apple-darwin --release --bin rune'
