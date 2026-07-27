THIS_MAKEFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
RUNE := "$(dir $(THIS_MAKEFILE_PATH))rune"

build:
	CGO_ENABLED=1 go build -ldflags "-s -w" -o $(RUNE) ./cmd/rune

run: build
	$(RUNE) $(ARGS)
rune: run

clean:
	rm -f $(RUNE)

test:
	go test -race -timeout 10m -coverprofile=coverage.out -covermode=atomic ./...
	go vet ./...
	go vet -tags fuzzing ./...

T ?= 1m
RC ?= 512
RS ?=

test-fuzz:
	go test -tags fuzzing -count=1 -fuzz='^FuzzBufferSnapshotImmutability$$' -fuzztime=$(T) ./pkg/editor/buffer
	go test -tags fuzzing -count=1 -fuzz='^FuzzBufferBatchEquivalence$$'      -fuzztime=$(T) ./pkg/editor/buffer
	go test -tags fuzzing -count=1 -fuzz='^FuzzBufferPointRoundtrip$$'        -fuzztime=$(T) ./pkg/editor/buffer
	go test -tags fuzzing -count=1 -fuzz='^FuzzSyntaxMapRoundtrip$$'          -fuzztime=$(T) ./pkg/editor/display
	go test -tags fuzzing -count=1 -fuzz='^FuzzWrapMapRoundtrip$$'            -fuzztime=$(T) ./pkg/editor/display
	go test -tags fuzzing -count=1 -fuzz='^FuzzEvictionModel$$'               -fuzztime=$(T) ./pkg/ui/components/opentabs
	go test -tags fuzzing -count=1 -fuzz='^FuzzSession$$'                     -fuzztime=$(T) ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzSessionWithFile$$'             -fuzztime=$(T) ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzWorkspaceTabOps$$'             -fuzztime=$(T) ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzLoadReorder$$'                 -fuzztime=$(T) ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzSaveRace$$'                    -fuzztime=$(T) ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzDelayedViewResult$$'           -fuzztime=$(T) ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzHumanSession$$'                -fuzztime=$(T) ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzTwoSessionsSharedDoc$$'        -fuzztime=$(T) ./pkg/ui/pages/workspace

release-snapshot:
	goreleaser release --snapshot --clean

whisper.cpp-restart:
	brew services restart whisper-cpp-server

.PHONY: build run test clean test-fuzz release-snapshot whisper.cpp-restart rust-build rust-test rust-lint rust-fmt rust-bench rust-perf-guard rust-test-fuzz parity parity-capture parity-diff parity-assert parity-serve parity-clean

rust-build:
	$(MAKE) -C rust build

rust-test:
	$(MAKE) -C rust test

rust-lint:
	$(MAKE) -C rust lint

rust-fmt:
	$(MAKE) -C rust fmt

rust-bench:
	$(MAKE) -C rust bench

rust-perf-guard:
	$(MAKE) -C rust perf-guard

rust-test-fuzz:
	$(MAKE) -C rust test-fuzz RC=$(RC) RS=$(RS)

PARITY_SCENARIO ?= 01-open-file

parity: parity-capture parity-diff parity-assert

parity-capture: build rust-build
	scripts/parity/capture.sh go   $(PARITY_SCENARIO)
	scripts/parity/capture.sh rust $(PARITY_SCENARIO)

parity-diff:
	scripts/parity/diff.sh

parity-assert:
	scripts/parity/assert.sh

parity-serve:
	scripts/parity/serve.sh go
	scripts/parity/serve.sh rust

parity-clean:
	scripts/parity/clean.sh

