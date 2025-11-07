# Makefile
SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c
IMAGE_NAME := zallet
IMAGE_TAG := latest

.PHONY: all build import
all: build import

.PHONY: build
build:
	@echo "Running compat check..."
	@out="$$(bash utils/compat.sh)"; \
	if [[ -z "$$out" ]]; then \
		echo "Compat produced no output; proceeding to build"; \
		bash utils/build.sh; \
	else \
		echo "Compat produced output; not building."; \
		printf '%s\n' "$$out"; \
		exit 1; \
	fi


.PHONY: import
import:
	docker load -i build/oci/zallet.tar
	docker tag $(IMAGE_NAME):latest $(IMAGE_NAME):$(IMAGE_TAG)
