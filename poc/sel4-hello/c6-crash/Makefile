# Jalon C.6-crash — validation P6 atomicité crash (ADR-0043)
#
# 4 kill points testent l'invariant Q3-C sous suspension forcée :
#   KP1 : après push blob, avant push header
#   KP2 : après push header, avant push log_entry
#   KP3 : après write log_entry + fence + data_len, avant ep.call()
#   KP4 : après retour de ep.call() (commit complet)
#
# Invariants vérifiés (oracle dans le serveur) :
#   I1 : journal_len ∈ {0, 1}
#   I2 : si journal_len > 0, headers >= journal_len && blobs >= journal_len
#   I3 : KP1/KP2/KP3 ⇒ journal_len=0 ; KP4 ⇒ journal_len=1
#
# Usage :
#   make build KP=1  — compile pour kill_point 1
#   make run KP=1    — compile + QEMU interactif
#   make test        — 4 builds + 4 QEMU tests → C6-crash_PASS
#   make clean       — supprime artefacts

DOCKER_IMAGE  := rust-root-task-demo
SEL4_PREFIX   := /opt/seL4
KP            ?= 1

# poc/ est deux niveaux au-dessus : poc/sel4-hello/c6-crash/ → poc/
POC_DIR       := $(abspath $(CURDIR)/../..)
DOCKER_C6CRASH := /poc/sel4-hello/c6-crash
BUILD          := $(DOCKER_C6CRASH)/build

SERVER_CRATE   := os-poc-c6crash-server
RUNTIME_CRATE  := os-poc-c6crash-runtime
SUPERVISOR_CRATE := os-poc-c6crash-supervisor

SERVER_ELF     := $(BUILD)/$(SERVER_CRATE).elf
RUNTIME_ELF    := $(BUILD)/$(RUNTIME_CRATE)-kp$(KP).elf
SUPERVISOR_ELF := $(BUILD)/$(SUPERVISOR_CRATE)-kp$(KP).elf
IMAGE          := $(BUILD)/image-kp$(KP).elf

LOADER         := $(SEL4_PREFIX)/bin/sel4-kernel-loader
LOADER_CLI     := $(SEL4_PREFIX)/bin/sel4-kernel-loader-add-payload

QEMU_CMD := qemu-system-aarch64 \
	-machine virt,virtualization=on \
	-cpu cortex-a57 \
	-m size=1G \
	-serial mon:stdio \
	-nographic \
	-kernel $(IMAGE)

CARGO_HOME ?= $(HOME)/.cargo

.PHONY: build run test clean _build _server _runtime _supervisor _image _run _test_kp

build:
	docker run --rm \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C6CRASH) \
		$(DOCKER_IMAGE) \
		make _build KP=$(KP) SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

run:
	docker run --rm -it \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C6CRASH) \
		$(DOCKER_IMAGE) \
		make _run KP=$(KP) SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

test:
	@for kp in 1 2 3 4; do \
		echo "[C6-crash] === Test KP$$kp ===" ; \
		docker run --rm -i \
			-v "$(POC_DIR):/poc" \
			-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
			-w $(DOCKER_C6CRASH) \
			$(DOCKER_IMAGE) \
			make _test_kp KP=$$kp SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD) \
		|| exit 1 ; \
	done
	@echo "C6-crash_PASS"

clean:
	docker run --rm \
		-v "$(POC_DIR):/poc" -w $(DOCKER_C6CRASH) \
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
	KILL_POINT=$(KP) SEL4_PREFIX=$(SEL4_PREFIX) cargo build -p $(RUNTIME_CRATE) \
		--target aarch64-sel4 \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		--release
	cp target/aarch64-sel4/release/$(RUNTIME_CRATE).elf $(RUNTIME_ELF)

_supervisor:
	KILL_POINT=$(KP) \
	SERVER_ELF=$(SERVER_ELF) \
	RUNTIME_ELF=$(RUNTIME_ELF) \
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

_test_kp: _build
	IMAGE=$(IMAGE) KP=$(KP) python3 test.py
