DEMO_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
FEATURES := explorer editing tables

.NOTPARALLEL:

all: $(FEATURES)

$(FEATURES):
	devbox run --config $(DEMO_DIR) -- $(DEMO_DIR)record.sh $@

check:
	devbox run --config $(DEMO_DIR) -- $(DEMO_DIR)record.sh --check

.PHONY: all check $(FEATURES)
