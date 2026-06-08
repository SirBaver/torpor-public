# Jalon C.5 — redb no_std sur sel4-virtio-blk (ADR-0042)
#
# Usage :
#   make build     — compile dans Docker (redb fork + root task)
#   make disk      — crée un disk.img de 64 MB (assez pour redb N=10K entrées)
#   make run       — compile + QEMU avec virtio-blk
#   make test      — compile + QEMU + vérifie C5_PASS
#   make clean     — supprime artefacts
#
# Note : le Makefile monte poc/ (pas seulement c5/) dans Docker pour que
# cargo puisse résoudre la dépendance path redb = { path = "../../redb-fork" }.

DOCKER_IMAGE  := rust-root-task-demo
SEL4_PREFIX   := /opt/seL4
APP_CRATE     := os-poc-c5-redb-on-virtio

# poc/ est deux niveaux au-dessus : poc/sel4-hello/c5-redb-on-virtio/ → poc/
POC_DIR       := $(abspath $(CURDIR)/../..)
# Chemin de c5/ depuis le conteneur Docker (poc/ monté à /poc)
DOCKER_C5     := /poc/sel4-hello/c5-redb-on-virtio
BUILD         := $(DOCKER_C5)/build
APP           := $(BUILD)/$(APP_CRATE).elf
IMAGE         := $(BUILD)/image.elf
LOADER        := $(SEL4_PREFIX)/bin/sel4-kernel-loader
LOADER_CLI    := $(SEL4_PREFIX)/bin/sel4-kernel-loader-add-payload
DISK_IMG      := $(CURDIR)/disk.img
DISK_IMG_DOCK := $(DOCKER_C5)/disk.img
DISK_SIZE_MB  := 8

QEMU_CMD := qemu-system-aarch64 \
	-machine virt,virtualization=on \
	-cpu cortex-a57 \
	-m size=1G \
	-serial mon:stdio \
	-nographic \
	-drive if=none,id=blk0,file=$(DISK_IMG_DOCK),format=raw \
	-device virtio-blk-device,drive=blk0 \
	-kernel $(IMAGE)

CARGO_HOME ?= $(HOME)/.cargo

.PHONY: build disk run test clean _build _app _image _run _test

disk:
	@if [ ! -f "$(DISK_IMG)" ]; then \
		echo "[C.5] Création disk.img ($(DISK_SIZE_MB) MB)..." ; \
		dd if=/dev/zero of=$(DISK_IMG) bs=1M count=$(DISK_SIZE_MB) 2>/dev/null ; \
		echo "[C.5] disk.img créé ($(DISK_SIZE_MB) MB)" ; \
	else \
		echo "[C.5] disk.img existe déjà ($(shell du -m $(DISK_IMG) | cut -f1) MB)" ; \
	fi

build: disk
	docker run --rm \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C5) \
		$(DOCKER_IMAGE) \
		make _build SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

run: disk
	docker run --rm -it \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C5) \
		$(DOCKER_IMAGE) \
		make _run SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

test: disk
	docker run --rm -i \
		-v "$(POC_DIR):/poc" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w $(DOCKER_C5) \
		$(DOCKER_IMAGE) \
		make _test SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

clean:
	rm -rf target/ $(DISK_IMG)
	docker run --rm \
		-v "$(POC_DIR):/poc" -w $(DOCKER_C5) \
		$(DOCKER_IMAGE) rm -rf $(BUILD)

_build: _app _image

_app:
	SEL4_PREFIX=$(SEL4_PREFIX) cargo build \
		--target aarch64-sel4 \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		--release
	mkdir -p $(BUILD)
	cp target/aarch64-sel4/release/$(APP_CRATE).elf $(APP)

_image:
	$(LOADER_CLI) \
		--loader $(LOADER) \
		--sel4-prefix $(SEL4_PREFIX) \
		--app $(APP) \
		-o $(IMAGE)

_run: _build
	$(QEMU_CMD)

_test: _build
	IMAGE=$(IMAGE) DISK=$(DISK_IMG_DOCK) python3 test.py
