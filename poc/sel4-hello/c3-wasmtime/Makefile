# Jalon C.3 — Root task seL4 + Wasmtime min-platform AArch64 (ADR-0037/0039)
#
# Stratégie AOT : build.rs compile WAT → add.cwasm sur le host (cranelift x86_64),
# la root task embarque add.cwasm et l'exécute via Module::deserialize (no_std).
#
# Usage depuis le host :
#   make build    — compile dans Docker (long à la 1ère fois : wasmtime build-dep)
#   make run      — compile + QEMU dans Docker (Ctrl-A X pour quitter)
#   make test     — compile + QEMU + vérifie C3_PASS (automatisé)
#   make clean    — supprime artefacts
#
# Pré-requis : image Docker rust-root-task-demo (voir poc/sel4-hello/run-c1.sh)
# Note : la 1ère compilation télécharge wasmtime (~100 crates) et prend ~10 min.

DOCKER_IMAGE  := rust-root-task-demo
SEL4_PREFIX   := /opt/seL4
BUILD         := /work/build
APP_CRATE     := os-poc-c3-wasmtime
APP           := $(BUILD)/$(APP_CRATE).elf
IMAGE         := $(BUILD)/image.elf
LOADER        := $(SEL4_PREFIX)/bin/sel4-kernel-loader
LOADER_CLI    := $(SEL4_PREFIX)/bin/sel4-kernel-loader-add-payload

QEMU_CMD := qemu-system-aarch64 \
	-machine virt,virtualization=on \
	-cpu cortex-a57 \
	-m size=1G \
	-serial mon:stdio \
	-nographic \
	-kernel $(IMAGE)

# Monte le registry cargo du host pour éviter un re-téléchargement dans Docker
CARGO_HOME ?= $(HOME)/.cargo

.PHONY: build run test clean _build _app _image _run _test

# ── Targets host (délèguent à Docker) ────────────────────────────────────────

build:
	docker run --rm \
		-v "$(CURDIR):/work" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _build SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

run:
	docker run --rm -it \
		-v "$(CURDIR):/work" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _run SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

test:
	docker run --rm -i \
		-v "$(CURDIR):/work" \
		-v "$(CARGO_HOME)/registry:/home/x/.cargo/registry" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _test SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

clean:
	rm -rf target/
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
