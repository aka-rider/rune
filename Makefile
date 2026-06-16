THIS_MAKEFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
RUNE := "$(dir $(THIS_MAKEFILE_PATH))rune"

build:
	CGO_CFLAGS="-I/opt/homebrew/include" \
	CGO_LDFLAGS="-L/opt/homebrew/lib" \
	CGO_ENABLED=1 go build -ldflags "-s -w" -o $(RUNE) ./cmd/rune

run: build
	$(RUNE) $(ARGS)
rune: run


test:
	go test -race -coverprofile=coverage.out -covermode=atomic ./...
	go vet ./...

test-fuzz-corpus:
	go test -tags fuzzing -run=FuzzSession ./pkg/ui/pages/workspace/

clean:
	rm -f $(RUNE)

.PHONY: build build-fuzz run test clean test-fuzz-corpus

