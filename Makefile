CARGO ?= cargo
# Randomized sessions per `make test-fuzz` (PROPTEST_CASES). An EMPTY value is
# not "unset" — proptest warns to stderr and silently falls back to 256
# (config.rs parse_or_warn) — so this default has to be spelled out.
RC ?= 512
# Optional PROPTEST_RNG_SEED for a pinned re-run. Empty = fresh OS entropy.
RS ?=

.PHONY: build test lint fmt bench perf-guard test-fuzz test-grammars go-build \
        parity parity-capture parity-diff parity-assert parity-grid parity-serve parity-clean \
        image-parity-dump

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

# `--test perf_guard` + `--exact` scope each invocation to ONE test in ONE
# binary. Without them, every other test binary prints `test result: ok. 0
# passed ... filtered out` and exits 0, so the target stays green even if the
# test is renamed or deleted — this is why the WP16 keystroke-latency guard
# below needs its OWN invocation rather than widening the `--exact` filter on
# this one, which by definition can never pick up a second test name.
perf-guard:
	$(CARGO) test -p rune-md --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 full_pipeline_5k_under_100ms
	$(CARGO) test -p rune-tui --release --test perf_guard -- \
	    --ignored --exact --test-threads=1 keystroke_view_cost_under_budget_on_a_5k_line_code_document

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

# The heavy per-grammar property test: every one of the 22 tree-sitter
# grammars against many arbitrary sources, not just the handful the
# non-ignored smoke test runs on every `make test`.
test-grammars:
	$(CARGO) test -p rune-ts --test grammar_props -- --ignored --test-threads=1

# ── Image byte-parity goldens ──────────────────────────────────────────────
# Regenerates crates/rune-image/tests/golden/ from the Go reference
# implementation's pure imagekit functions. imagekit is pure Go, so this
# needs no CGO. The goldens are committed; re-run this only when the Go
# reference behaviour intentionally changes.
image-parity-dump:
	mkdir -p crates/rune-image/tests/golden
	cd golang && go run ./cmd/imgdump pure > ../crates/rune-image/tests/golden/pure.json
	cd golang && go run ./cmd/imgdump encode testdata/assets/x.png     1  8  3 \
	    > ../crates/rune-image/tests/golden/encode_x_png.json
	cd golang && go run ./cmd/imgdump encode testdata/assets/y.png     2  5  3 \
	    > ../crates/rune-image/tests/golden/encode_y_png.json
	cd golang && go run ./cmd/imgdump encode testdata/assets/photo.jpg 3 10  4 \
	    > ../crates/rune-image/tests/golden/encode_photo_jpg.json
	cd golang && go run ./cmd/imgdump encode testdata/assets/anim.gif  4  6  3 \
	    > ../crates/rune-image/tests/golden/encode_anim_gif.json
	cd golang && go run ./cmd/imgdump encode testdata/assets/x.png     5 80 40 \
	    > ../crates/rune-image/tests/golden/encode_x_png_upscale.json
	# noise.png is incompressible by construction, so its PNG payload's
	# base64 spans several 4096-char chunks — the only fixture that
	# exercises multi-chunk APC framing (m=1 / m=0) against the reference.
	cd golang && go run ./cmd/imgdump encode testdata/assets/noise.png 6  8  4 \
	    > ../crates/rune-image/tests/golden/encode_noise_png.json
	cd golang && go run ./cmd/imgdump delete 42 \
	    > ../crates/rune-image/tests/golden/delete.json
	cd golang && go run ./cmd/imgdump delete-all \
	    > ../crates/rune-image/tests/golden/delete_all.json

# ── Parity harness ────────────────────────────────────────────────────────────
# Captures the same scenario from both implementations and diffs the screens.
# The Go reference implementation lives in golang/ and builds with its own
# Makefile; `go`/`rust` below are side NAMES the scripts dispatch on.

PARITY_SCENARIO ?= 01-open-file

parity: parity-capture parity-diff parity-assert

go-build:
	$(MAKE) -C golang build

parity-capture: build go-build
	scripts/parity/capture.sh go   $(PARITY_SCENARIO)
	scripts/parity/capture.sh rust $(PARITY_SCENARIO)

parity-diff:
	scripts/parity/diff.sh

parity-assert:
	scripts/parity/assert.sh

parity-grid: build go-build
	scripts/parity/grid.sh

parity-serve:
	scripts/parity/serve.sh go
	scripts/parity/serve.sh rust

parity-clean:
	scripts/parity/clean.sh
