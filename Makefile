# preemptive-threads: build, run and test targets.
#
#   make test            run the test kernel on every emulated machine
#   make test-virt       QEMU virt, entered at EL1 (PSCI over HVC)
#   make test-virt-el2   QEMU virt with virtualization=on (EL2 -> EL1 drop, PSCI over SMC)
#   make test-raspi3b    QEMU raspi3b booting a flat image through the spin-table stub at EL2
#   make kernel8.img     demo kernel image for a Raspberry Pi Zero 2 W SD card
#   make run-raspi3b     run the demo on QEMU raspi3b (Ctrl-C to quit)
#   make run-virt        run the demo on QEMU virt
#   make clippy          lint both board configurations

CARGO        ?= cargo
JOBS         ?= 4
TARGET       := aarch64-unknown-none
TIMEOUT      ?= 180
OBJCOPY      ?= $(shell ls $$(rustc --print sysroot)/lib/rustlib/*/bin/rust-objcopy 2>/dev/null | head -1)

RPI_DIR      := target/rpi
VIRT_DIR     := target/virt
RPI_OUT      := $(RPI_DIR)/$(TARGET)/release/examples
VIRT_OUT     := $(VIRT_DIR)/$(TARGET)/release/examples
PASS_MARKER  := ALL [0-9]+ TESTS PASSED on 4 cores

.PHONY: all build test test-virt test-virt-el2 test-raspi3b run-raspi3b run-virt clippy doc clean \
        build/test-virt.elf build/test-raspi3b.img kernel8.img build/demo-virt.elf

all: build

build:
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR) --examples
	$(CARGO) build -j$(JOBS) --release --target-dir $(VIRT_DIR) --examples --features qemu-virt

# --- test kernels ---------------------------------------------------------

build/test-virt.elf:
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(VIRT_DIR) --example test_kernel --features qemu-virt,semihosting
	cp $(VIRT_OUT)/test_kernel $@

build/test-raspi3b.img:
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR) --example test_kernel --features semihosting
	$(OBJCOPY) -O binary $(RPI_OUT)/test_kernel $@

test-virt: build/test-virt.elf
	scripts/qemu-run.sh virt $< $(TIMEOUT) build/test-virt.log
	grep -Eq "$(PASS_MARKER)" build/test-virt.log

test-virt-el2: build/test-virt.elf
	scripts/qemu-run.sh virt-el2 $< $(TIMEOUT) build/test-virt-el2.log
	grep -Eq "$(PASS_MARKER)" build/test-virt-el2.log

test-raspi3b: build/test-raspi3b.img
	scripts/qemu-run.sh raspi3b $< $(TIMEOUT) build/test-raspi3b.log
	grep -Eq "$(PASS_MARKER)" build/test-raspi3b.log

test: test-virt test-virt-el2 test-raspi3b

# --- demo kernel ----------------------------------------------------------

kernel8.img:
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR) --example rpi_kernel
	$(OBJCOPY) -O binary $(RPI_OUT)/rpi_kernel $@
	@echo "kernel8.img ready: copy it to the SD card boot partition (see README)"

build/demo-virt.elf:
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(VIRT_DIR) --example rpi_kernel --features qemu-virt
	cp $(VIRT_OUT)/rpi_kernel $@

run-raspi3b: kernel8.img
	qemu-system-aarch64 -M raspi3b -display none -serial stdio -kernel kernel8.img

run-virt: build/demo-virt.elf
	qemu-system-aarch64 -M virt,gic-version=2 -cpu cortex-a53 -smp 4 -m 512M -display none -serial stdio -kernel $<

# --- lint / docs ----------------------------------------------------------

clippy:
	$(CARGO) clippy -j$(JOBS) --release --target-dir $(RPI_DIR) --examples --features semihosting -- -D warnings
	$(CARGO) clippy -j$(JOBS) --release --target-dir $(VIRT_DIR) --examples --features qemu-virt,semihosting -- -D warnings

doc:
	$(CARGO) doc -j$(JOBS) --no-deps --target-dir $(RPI_DIR)

clean:
	$(CARGO) clean
	rm -f kernel8.img build/*.elf build/*.img build/*.log
