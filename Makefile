CARGO ?= cargo
# Randomized sessions per `make test-fuzz` (PROPTEST_CASES). An EMPTY value is
# not "unset" — proptest warns to stderr and silently falls back to 256
# (config.rs parse_or_warn) — so this default has to be spelled out.
RC ?= 512
# Optional PROPTEST_RNG_SEED for a pinned re-run. Empty = fresh OS entropy.
RS ?=
# cargo-mutants parallel jobs. Conservative on purpose: each job cold-builds
# its own tree copy (target/ is never copied), so more jobs mostly burn disk.
J ?= 2
# Extra cargo-mutants args, e.g. MUTANTS_ARGS='--iterate'.
MUTANTS_ARGS ?=
# nextest profile (see .config/nextest.toml), not a cargo profile. CI passes
# PROFILE=ci. Applies to the `test` target only.
PROFILE ?= default

.PHONY: build test lint fmt bench perf-guard test-fuzz test-grammars mutants \
        cross-compile-from-linux-to-macos yo-build-and-fetch

SDK_CACHE := $(HOME)/.cache/rune/MacOSX14.5.sdk
MAC_BIN := target/aarch64-apple-darwin/release/rune
# The Linux VM that produces the macOS binary, and its checkout of this repo.
YO ?= yolobox
YO_REPO ?= wrk/rune

build:
	$(CARGO) build --workspace

test:
	$(CARGO) nextest run --profile $(PROFILE) --workspace
	# Cargo feature unification already arms strict-invariants on rune-md for the
	# whole --workspace run above (rune-fuzz requests it), but this isolated
	# invocation stays so the suite still proves out if that unification ever goes away.
	$(CARGO) nextest run --profile $(PROFILE) -p rune-md --features strict-invariants

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) clippy --workspace --lib -- -D warnings

fmt:
	$(CARGO) fmt --all --check

bench:
	$(CARGO) bench -p rune-md --bench parse_bench

# Each guard keeps its own invocation: a merged filterset like
# `test(=a) + test(=b)` is a union that still passes if `b` is deleted.
# nextest exits 4 (NO_TESTS_RUN) when a filter matches nothing, so one
# invocation per test is what makes a renamed or deleted guard fail loudly.
# `-p <crate>` stays on every line: `-E` selects tests, not build targets, so
# dropping it would widen the release build from one crate to the workspace.
perf-guard:
	$(CARGO) nextest run -p rune-md --release --test perf_guard --run-ignored only -E 'test(=full_pipeline_5k_under_100ms)'
	$(CARGO) nextest run -p rune-md --release --test perf_guard --run-ignored only -E 'test(=full_pipeline_cost_scales_linearly_not_quadratically_with_document_size)'
	$(CARGO) nextest run -p rune-tui --release --test perf_guard --run-ignored only -E 'test(=keystroke_view_cost_under_budget_on_a_5k_line_code_document)'
	$(CARGO) nextest run -p rune-tui --release --test perf_guard --run-ignored only -E 'test(=render_frame_cost_under_budget_on_a_5k_line_code_document)'
	$(CARGO) nextest run -p rune-tui --release --test perf_guard --run-ignored only -E 'test(=render_frame_cost_under_budget_on_a_many_fence_markdown_document)'
	$(CARGO) nextest run -p rune-tui --release --test perf_guard --run-ignored only -E 'test(=render_frame_cost_under_budget_with_the_caret_on_an_unmatched_bracket)'
	$(CARGO) nextest run -p rune-tui --release --test perf_guard --run-ignored only -E 'test(=bootstrap_first_draw_stays_bounded_on_a_large_document)'

# One invocation, one test, for the same reason as perf-guard.
# Debug profile on purpose: keeps the buffer/undo/render debug_asserts armed.
test-fuzz:
	PROPTEST_CASES=$(RC) PROPTEST_RNG_SEED=$(RS) \
	    $(CARGO) nextest run -p rune-fuzz --test human_session \
	    --run-ignored only -E 'test(=human_session)'

# The heavy per-grammar property test: every one of the 32 tree-sitter
# grammars against many arbitrary sources, not just the handful the
# non-ignored smoke test runs on every `make test`.
test-grammars:
	$(CARGO) nextest run -p rune-ts --test grammar_props --run-ignored only

# Mutation testing (cargo-mutants, config in .cargo/mutants.toml).
# PKG=<crate> scopes the run to one package.
mutants:
	$(CARGO) mutants $(if $(PKG),--package $(PKG)) --jobs $(J) $(MUTANTS_ARGS)

# The macOS binary is built on Linux (the yolobox guest): zig is the cross
# linker, the Apple SDK is cached under $(SDK_CACHE).
cross-compile-from-linux-to-macos:
	command -v nix >/dev/null || { echo "cross-compile-from-linux-to-macos needs nix on PATH"; exit 1; }
	[ -d $(SDK_CACHE) ] || { \
	    mkdir -p $(HOME)/.cache/rune; \
	    curl -fL https://github.com/joseluisq/macosx-sdks/releases/download/14.5/MacOSX14.5.sdk.tar.xz \
	        | tar -xJ -C $(HOME)/.cache/rune; \
	}
	nix shell nixpkgs#rustup nixpkgs#gcc nixpkgs#zig nixpkgs#cargo-zigbuild -c \
	    sh -c 'rustup target add aarch64-apple-darwin && \
	        SDKROOT=$(SDK_CACHE) cargo zigbuild --target aarch64-apple-darwin --release --bin rune'

# Host side of the same build: drive the guest, then pull the binary back.
# The guest holds its own clone, so bring it to the commit you want first.
yo-build-and-fetch:
	limactl shell --workdir $(YO_REPO) $(YO) nix shell nixpkgs#gnumake -c \
	    make cross-compile-from-linux-to-macos
	mkdir -p $(dir $(MAC_BIN))
	limactl copy $(YO):$(YO_REPO)/$(MAC_BIN) $(MAC_BIN)
	chmod +x $(MAC_BIN)
	@echo "fetched $(MAC_BIN)"
