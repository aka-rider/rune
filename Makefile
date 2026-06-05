THIS_MAKEFILE_PATH := $(abspath $(lastword $(MAKEFILE_LIST)))
RUNE := "$(dir $(THIS_MAKEFILE_PATH))rune"

build:
	go build -ldflags "-s -w" -o $(RUNE) ./cmd/rune/main.go

run: build
	$(RUNE) $(ARGS)
rune: run


test:
	go test -race -coverprofile=coverage.out -covermode=atomic ./...
	go vet ./...

clean:
	rm -f $(BINARY)

.PHONY: build run test clean

