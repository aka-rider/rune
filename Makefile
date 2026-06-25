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
	go test -race -coverprofile=coverage.out -covermode=atomic ./...
	go vet ./...

T ?= 1m

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

release-snapshot:
	goreleaser release --snapshot --clean

whisper.cpp-restart:
	brew services restart whisper-cpp-server

.PHONY: build build-fuzz run test clean test-fuzz release-snapshot whisper.cpp-restart

