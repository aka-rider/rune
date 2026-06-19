THIS_MAKEFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
RUNE := "$(dir $(THIS_MAKEFILE_PATH))rune"

build:
	CGO_CFLAGS="-I/opt/homebrew/include" \
	CGO_LDFLAGS="-L/opt/homebrew/lib" \
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
	go test -tags fuzzing -count=1 -run='Fuzz' ./...

whisper.cpp-restart:
	brew services restart whisper-cpp-server

.PHONY: build build-fuzz run test clean test-fuzz whisper.cpp-restart

