# Jalon C.2 — Root task custom minimale seL4 AArch64 (ADR-0039)
# Requiert : image Docker rust-root-task-demo (Jalon C.1 complété avec run-c1.sh)
#
# Usage depuis le host :
#   make build   — compile dans Docker
#   make run     — compile + QEMU dans Docker (Ctrl-A X pour quitter)
#   make test    — compile + QEMU + vérifie C2_PASS (automatisé)
#   make clean   — supprime artefacts

DOCKER_IMAGE  := rust-root-task-demo
SEL4_PREFIX   := /opt/seL4
BUILD         := /tmp/c2-build
APP_CRATE     := os-poc-sel4-hello
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

.PHONY: build run test clean _build _app _image _run _test

# ── Targets host (délèguent à Docker) ────────────────────────────────────────

build:
	docker run --rm \
		-v "$(CURDIR):/work" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _build SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

run:
	docker run --rm -it \
		-v "$(CURDIR):/work" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _run SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

test:
	docker run --rm -i \
		-v "$(CURDIR):/work" \
		-w /work \
		$(DOCKER_IMAGE) \
		$(MAKE) _test SEL4_PREFIX=$(SEL4_PREFIX) BUILD=$(BUILD)

clean:
	rm -rf target/
	docker run --rm -v "$(CURDIR):/work" -w /work $(DOCKER_IMAGE) rm -rf $(BUILD)

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
