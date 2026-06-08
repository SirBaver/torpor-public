# Jalon C.4 — Root task seL4 + driver virtio-blk (ADR-0041)
#
# Usage :
#   make build     — compile dans Docker
#   make disk      — crée un disk.img de test (1 MB, bloc 0 = 0x00 initialement)
#   make run       — compile + QEMU avec virtio-blk (Ctrl-A X pour quitter)
#   make test      — compile + QEMU + vérifie C4_PASS
#   make clean     — supprime artefacts

DOCKER_IMAGE  := rust-root-task-demo
SEL4_PREFIX   := /opt/seL4
BUILD         := /work/build
APP_CRATE     := os-poc-c4-virtio-blk
APP           := $(BUILD)/$(APP_CRATE).elf
IMAGE         := $(BUILD)/image.elf
LOADER        := $(SEL4_PREFIX)/bin/sel4-kernel-loader
LOADER_CLI    := $(SEL4_PREFIX)/bin/sel4-kernel-loader-add-payload
DISK_IMG      := $(CURDIR)/disk.img

QEMU_CMD := qemu-system-aarch64 \
	-machine virt,virtualization=on \
	-cpu cortex-a57 \
	-m size=1G \
	-serial mon:stdio \
	-nographic \
	-drive if=none,id=blk0,file=$(DISK_IMG),format=raw \
	-device virtio-blk-device,drive=blk0 \
	-kernel $(IMAGE)

CARGO_HOME ?= $(HOME)/.cargo

.PHONY: build disk run test clean _build _app _image _run _test

# ── Targets host ──────────────────────────────────────────────────────────────

disk:
	@if [ ! -f "$(DISK_IMG)" ]; then \
		echo "[C.4] Création disk.img (1 MB)..." ; \
		dd if=/dev/zero of=$(DISK_IMG) bs=1M count=1 2>/dev/null ; \
		echo "[C.4] disk.img créé" ; \
	else \
		echo "[C.4] disk.img existe déjà" ; \
	fi

build: disk
	docker run --rm \
		-v "$(CURDIR):/work" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _build SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

run: disk
	docker run --rm -it \
		-v "$(CURDIR):/work" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _run SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

test: disk
	docker run --rm -i \
		-v "$(CURDIR):/work" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _test SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

clean:
	rm -rf target/ disk.img
	docker run --rm \
		-v "$(CURDIR):/work" -w /work \
		$(DOCKER_IMAGE) rm -rf $(BUILD)

# ── Targets internes (dans Docker) ───────────────────────────────────────────

_build: _image

_run: _image
	$(QEMU_CMD)

_test: _image
	python3 test.py "$(QEMU_CMD)"

_image: _app
	$(LOADER_CLI) \
		--loader $(LOADER) \
		--sel4-prefix $(SEL4_PREFIX) \
		--app $(APP) \
		-o $(IMAGE)

_app:
	mkdir -p $(BUILD)
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build \
		--target aarch64-sel4 \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		--release
	cp target/aarch64-sel4/release/$(APP_CRATE).elf $(APP)
