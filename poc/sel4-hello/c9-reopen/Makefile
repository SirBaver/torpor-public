# Jalon C.9 — Smoke test persistance seL4 (D-reopen, ADR-0046)
#
# Deux runs QEMU sur le même disk.img (pas de dd entre les deux) :
#   Phase A : K=100 commits redb/virtio-blk → REOPEN_A_PASS
#   Phase B : reopen disk.img → vérification K entrées → C9_PASS
#
# Usage :
#   make build-a   — compile image Phase A
#   make build-b   — compile image Phase B
#   make test      — build-a + run Phase A, puis build-b + run Phase B → C9_PASS

DOCKER_IMAGE  := rust-root-task-demo
SEL4_PREFIX   := /opt/seL4

POC_DIR          := $(abspath $(CURDIR)/../..)
DOCKER_C9        := /poc/sel4-hello/c9-reopen
BUILD            := $(DOCKER_C9)/build
DISK_IMG         := $(BUILD)/disk.img

SERVER_CRATE     := os-poc-c9-reopen-server
RUNTIME_CRATE    := os-poc-c9-reopen-runtime
SUPERVISOR_CRATE := os-poc-c9-reopen-supervisor

SERVER_ELF       := $(BUILD)/$(SERVER_CRATE).elf
RUNTIME_ELF      := $(BUILD)/$(RUNTIME_CRATE).elf
SUPERVISOR_A_ELF := $(BUILD)/$(SUPERVISOR_CRATE)-phase-a.elf
SUPERVISOR_B_ELF := $(BUILD)/$(SUPERVISOR_CRATE)-phase-b.elf
IMAGE_A          := $(BUILD)/image-phase-a.elf
IMAGE_B          := $(BUILD)/image-phase-b.elf

LOADER     := $(SEL4_PREFIX)/bin/sel4-kernel-loader
LOADER_CLI := $(SEL4_PREFIX)/bin/sel4-kernel-loader-add-payload

QEMU_BASE := qemu-system-aarch64 \
	-machine virt,virtualization=on \
	-cpu cortex-a57 \
	-m size=1G \
	-serial mon:stdio \
	-nographic \
	-drive if=none,id=blk0,file=$(DISK_IMG),format=raw,cache=none,aio=native \
	-device virtio-blk-device,drive=blk0

CARGO_HOME ?= $(HOME)/.cargo

.PHONY: build-a build-b test clean _build-server _build-runtime _build-sup-a _build-sup-b \
        _image-a _image-b _disk _run-a _run-b _test

build-a:
	docker run --rm \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C9) \
		$(DOCKER_IMAGE) \
		make _build-a SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

build-b:
	docker run --rm \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C9) \
		$(DOCKER_IMAGE) \
		make _build-b SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

test:
	@echo "[C9] === Phase A — écriture K=100 commits ==="
	@docker run --rm -i \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C9) \
		$(DOCKER_IMAGE) \
		make _test SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

clean:
	rm -rf build/

# ── Targets internes (dans Docker) ──────────────────────────────────────────────

_build-a: _build-server _build-runtime _build-sup-a _image-a

_build-b: _build-server _build-sup-b _image-b

_test: _build-server _build-runtime _build-sup-a _image-a _run-a \
       _build-sup-b _image-b _run-b

_build-server:
	mkdir -p $(BUILD)
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build \
		--release \
		-p $(SERVER_CRATE) \
		2>&1 | tail -5
	cp target/aarch64-sel4/release/$(SERVER_CRATE).elf $(SERVER_ELF)

_build-runtime:
	mkdir -p $(BUILD)
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build \
		--release \
		-p $(RUNTIME_CRATE) \
		2>&1 | tail -5
	cp target/aarch64-sel4/release/$(RUNTIME_CRATE).elf $(RUNTIME_ELF)

_build-sup-a:
	mkdir -p $(BUILD)
	SERVER_ELF=$(SERVER_ELF) RUNTIME_ELF=$(RUNTIME_ELF) PHASE=0 \
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build \
		--release \
		-p $(SUPERVISOR_CRATE) \
		2>&1 | tail -5
	cp target/aarch64-sel4/release/$(SUPERVISOR_CRATE).elf $(SUPERVISOR_A_ELF)

_build-sup-b:
	mkdir -p $(BUILD)
	SERVER_ELF=$(SERVER_ELF) RUNTIME_ELF=$(RUNTIME_ELF) PHASE=1 \
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build \
		--release \
		-p $(SUPERVISOR_CRATE) \
		2>&1 | tail -5
	cp target/aarch64-sel4/release/$(SUPERVISOR_CRATE).elf $(SUPERVISOR_B_ELF)

_image-a:
	$(LOADER_CLI) \
		--loader $(LOADER) \
		--sel4-prefix $(SEL4_PREFIX) \
		--app $(SUPERVISOR_A_ELF) \
		-o $(IMAGE_A)

_image-b:
	$(LOADER_CLI) \
		--loader $(LOADER) \
		--sel4-prefix $(SEL4_PREFIX) \
		--app $(SUPERVISOR_B_ELF) \
		-o $(IMAGE_B)

_run-a:
	BUILD=$(BUILD) IMAGE_A=$(IMAGE_A) DISK_IMG=$(DISK_IMG) python3 test.py phase-a

_run-b:
	BUILD=$(BUILD) IMAGE_B=$(IMAGE_B) DISK_IMG=$(DISK_IMG) python3 test.py phase-b
