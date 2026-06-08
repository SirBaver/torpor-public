# D-P3a — latence P3a sous seL4 (ADR-0045 amendement Q1, ADR-0046)
#
# Usage :
#   make test      — build dans Docker + QEMU + vérifie D_P3A_PASS/FAIL
#   make build     — compile seulement (dans Docker)
#   make clean     — supprime artefacts
#
# Prérequis : image Docker rust-root-task-demo construite (make -C c1-hello/).
# Durée estimée : 15–30 min (population 10^6 entrées via O_DIRECT + 3 passes).

DOCKER_IMAGE  := rust-root-task-demo
SEL4_PREFIX   := /opt/seL4
APP_CRATE     := os-poc-d-p3a-measure

POC_DIR       := $(abspath $(CURDIR)/../..)
DOCKER_DP3A   := /poc/sel4-hello/d-p3a
BUILD         := $(DOCKER_DP3A)/build
APP_ELF       := $(BUILD)/$(APP_CRATE).elf
IMAGE         := $(BUILD)/image.elf
LOADER        := $(SEL4_PREFIX)/bin/sel4-kernel-loader
LOADER_CLI    := $(SEL4_PREFIX)/bin/sel4-kernel-loader-add-payload
DISK_IMG      := $(CURDIR)/disk.img
DISK_IMG_DOCK := $(DOCKER_DP3A)/disk.img
DISK_SIZE_MB  := 512

CARGO_HOME ?= $(HOME)/.cargo

.PHONY: test build clean _build _app _image _test

disk:
	@if [ ! -f "$(DISK_IMG)" ]; then \
		echo "[d-p3a] Création disk.img ($(DISK_SIZE_MB) MB)..." ; \
		dd if=/dev/zero of=$(DISK_IMG) bs=1M count=$(DISK_SIZE_MB) 2>/dev/null ; \
		echo "[d-p3a] disk.img créé" ; \
	fi

build: disk
	docker run --rm \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_DP3A) \
		$(DOCKER_IMAGE) \
		make _build SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

test: disk
	docker run --rm -i \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_DP3A) \
		$(DOCKER_IMAGE) \
		make _test SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

clean:
	rm -rf target/ $(DISK_IMG)
	docker run --rm \
		-v "$(POC_DIR):/poc" -w $(DOCKER_DP3A) \
		$(DOCKER_IMAGE) rm -rf $(BUILD)

# ── Targets internes (dans Docker) ───────────────────────────────────────────

_build: _app _image

_app:
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build \
		--target aarch64-sel4 \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		--release
	mkdir -p $(BUILD)
	cp target/aarch64-sel4/release/$(APP_CRATE).elf $(APP_ELF)

_image:
	$(LOADER_CLI) \
		--loader $(LOADER) \
		--sel4-prefix $(SEL4_PREFIX) \
		--app $(APP_ELF) \
		-o $(IMAGE)

_test: _build
	IMAGE=$(IMAGE) DISK=$(DISK_IMG_DOCK) python3 test.py
