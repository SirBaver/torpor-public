# Jalon C.7-A — intégration N=2 agents seL4 AArch64 (ADR-0044)
#
# Topologie : supervisor (root task) + server (VSpace B) + 2×runtime (VSpaces A1/A2)
# N rings SPSC, dispatch par badge = agent_id, index (agent_id, k) dans le server
# Critère de succès : UART QEMU affiche C7-A_PASS
#
# Usage :
#   make build   — compile les 3 crates dans Docker
#   make run     — compile + QEMU interactif
#   make test    — compile + QEMU + vérifie C7-A_PASS
#   make clean   — supprime artefacts

DOCKER_IMAGE  := rust-root-task-demo
SEL4_PREFIX   := /opt/seL4

# poc/ est deux niveaux au-dessus : poc/sel4-hello/c7-integration/ → poc/
POC_DIR       := $(abspath $(CURDIR)/../..)
DOCKER_C7     := /poc/sel4-hello/c7-integration
BUILD         := $(DOCKER_C7)/build

SERVER_CRATE  := os-poc-c7-server
RUNTIME_CRATE := os-poc-c7-runtime
SUPERVISOR_CRATE := os-poc-c7-supervisor

SERVER_ELF    := $(BUILD)/$(SERVER_CRATE).elf
RUNTIME_ELF   := $(BUILD)/$(RUNTIME_CRATE).elf
SUPERVISOR_ELF := $(BUILD)/$(SUPERVISOR_CRATE).elf
IMAGE         := $(BUILD)/image.elf

LOADER        := $(SEL4_PREFIX)/bin/sel4-kernel-loader
LOADER_CLI    := $(SEL4_PREFIX)/bin/sel4-kernel-loader-add-payload

# QEMU sans virtio-blk (C.6 utilise seulement la RAM)
QEMU_CMD := qemu-system-aarch64 \
	-machine virt,virtualization=on \
	-cpu cortex-a57 \
	-m size=1G \
	-serial mon:stdio \
	-nographic \
	-kernel $(IMAGE)

CARGO_HOME ?= $(HOME)/.cargo

.PHONY: build run test clean _build _server _runtime _supervisor _image _run _test

build:
	docker run --rm \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C7) \
		$(DOCKER_IMAGE) \
		make _build SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

run:
	docker run --rm -it \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C7) \
		$(DOCKER_IMAGE) \
		make _run SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

test:
	docker run --rm -i \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C7) \
		$(DOCKER_IMAGE) \
		make _test SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

clean:
	docker run --rm \
		-v "$(POC_DIR):/poc" -w $(DOCKER_C7) \
		$(DOCKER_IMAGE) rm -rf $(BUILD)

_build: _server _runtime _supervisor _image

_server:
	mkdir -p $(BUILD)
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build -p $(SERVER_CRATE) \
		--target aarch64-sel4 \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		--release
	cp target/aarch64-sel4/release/$(SERVER_CRATE).elf $(SERVER_ELF)

_runtime:
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build -p $(RUNTIME_CRATE) \
		--target aarch64-sel4 \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		--release
	cp target/aarch64-sel4/release/$(RUNTIME_CRATE).elf $(RUNTIME_ELF)

_supervisor:
	SERVER_ELF=$(SERVER_ELF) RUNTIME_ELF=$(RUNTIME_ELF) \
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build -p $(SUPERVISOR_CRATE) \
		--target aarch64-sel4 \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		--release
	cp target/aarch64-sel4/release/$(SUPERVISOR_CRATE).elf $(SUPERVISOR_ELF)

_image:
	$(LOADER_CLI) \
		--loader $(LOADER) \
		--sel4-prefix $(SEL4_PREFIX) \
		--app $(SUPERVISOR_ELF) \
		-o $(IMAGE)

_run: _build
	$(QEMU_CMD)

_test: _build
	IMAGE=$(IMAGE) EXPECTED=C7-A_PASS python3 test.py
