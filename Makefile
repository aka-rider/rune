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

test-fuzz:
	go test -tags fuzzing -count=1 -fuzz='^FuzzBufferSnapshotImmutability$$' -fuzztime=1m ./pkg/editor/buffer
	go test -tags fuzzing -count=1 -fuzz='^FuzzBufferBatchEquivalence$$'      -fuzztime=1m ./pkg/editor/buffer
	go test -tags fuzzing -count=1 -fuzz='^FuzzBufferPointRoundtrip$$'        -fuzztime=1m ./pkg/editor/buffer
	go test -tags fuzzing -count=1 -fuzz='^FuzzSyntaxMapRoundtrip$$'          -fuzztime=1m ./pkg/editor/display
	go test -tags fuzzing -count=1 -fuzz='^FuzzWrapMapRoundtrip$$'            -fuzztime=1m ./pkg/editor/display
	go test -tags fuzzing -count=1 -fuzz='^FuzzEvictionModel$$'               -fuzztime=1m ./pkg/ui/components/opentabs
	go test -tags fuzzing -count=1 -fuzz='^FuzzSession$$'                     -fuzztime=1m ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzSessionWithFile$$'             -fuzztime=1m ./pkg/ui/pages/workspace
	go test -tags fuzzing -count=1 -fuzz='^FuzzWorkspaceTabOps$$'             -fuzztime=1m ./pkg/ui/pages/workspace

whisper.cpp-restart:
	brew services restart whisper-cpp-server

.PHONY: build build-fuzz run test clean test-fuzz whisper.cpp-restart

