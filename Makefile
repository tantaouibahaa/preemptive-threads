# Configuration
TARGET       := aarch64-unknown-none
PROFILE      := release
TOOLCHAIN    := +nightly

# Examples
EXAMPLE_FCFS := fcfs_kernel
EXAMPLE_RPI  := rpi_kernel

# Build paths
BUILD_DIR    := target/$(TARGET)/$(PROFILE)/examples
KERNEL_FCFS  := $(BUILD_DIR)/$(EXAMPLE_FCFS)
KERNEL_RPI   := $(BUILD_DIR)/$(EXAMPLE_RPI)
OUTPUT_BIN   := kernel8.img

# QEMU settings
QEMU         := qemu-system-aarch64
QEMU_FLAGS   := -serial stdio -display none
QEMU_PI_MACHINE     := raspi3b
QEMU_VIRT_MACHINE   := virt,gic-version=2
QEMU_VIRT_CPU       := cortex-a53
QEMU_DEBUG_FLAGS    := -d int,cpu_reset
QEMU_GDB_FLAGS      := -S -s

# Linker script for QEMU virt
VIRT_LINKER  := qemu_virt.ld

.PHONY: all build build-rpi build-virt run run-rpi run-virt debug debug-virt gdb binary disasm clean help

all: build

help:
	@echo "Available targets:"
	@echo "  build       - Build FCFS kernel for Raspberry Pi"
	@echo "  build-rpi   - Build RPI-specific kernel"
	@echo "  build-virt  - Build kernel for QEMU virt machine"
	@echo "  run         - Build and run FCFS kernel on QEMU (raspi3b)"
	@echo "  run-rpi     - Build and run RPI kernel on QEMU"
	@echo "  run-virt    - Build and run on QEMU virt machine"
	@echo "  debug       - Run with interrupt/reset debugging"
	@echo "  debug-virt  - Run virt machine with debugging"
	@echo "  gdb         - Run and wait for GDB connection"
	@echo "  binary      - Create flashable binary image"
	@echo "  disasm      - Show disassembly of kernel"
	@echo "  clean       - Remove build artifacts"

build:
	cargo $(TOOLCHAIN) build --$(PROFILE) --example $(EXAMPLE_FCFS) --target $(TARGET)

build-rpi:
	cargo $(TOOLCHAIN) build --$(PROFILE) --example $(EXAMPLE_RPI) --target $(TARGET)

build-virt:
	RUSTFLAGS="-C link-arg=-T$(VIRT_LINKER)" \
		cargo $(TOOLCHAIN) build --$(PROFILE) --example $(EXAMPLE_FCFS) --target $(TARGET) --features qemu-virt

run: build
	$(QEMU) -M $(QEMU_PI_MACHINE) -kernel $(KERNEL_FCFS) $(QEMU_FLAGS)

run-rpi: build-rpi
	$(QEMU) -M $(QEMU_PI_MACHINE) -kernel $(KERNEL_RPI) $(QEMU_FLAGS)

run-virt: build-virt
	$(QEMU) -M $(QEMU_VIRT_MACHINE) -cpu $(QEMU_VIRT_CPU) -kernel $(KERNEL_FCFS) $(QEMU_FLAGS)

debug: build
	$(QEMU) -M $(QEMU_PI_MACHINE) -kernel $(KERNEL_FCFS) $(QEMU_FLAGS) $(QEMU_DEBUG_FLAGS)

debug-virt: build-virt
	$(QEMU) -M $(QEMU_VIRT_MACHINE) -cpu $(QEMU_VIRT_CPU) -kernel $(KERNEL_FCFS) $(QEMU_FLAGS) $(QEMU_DEBUG_FLAGS)

gdb: build
	$(QEMU) -M $(QEMU_PI_MACHINE) -kernel $(KERNEL_FCFS) $(QEMU_FLAGS) $(QEMU_GDB_FLAGS)

binary: build
	rust-objcopy -O binary $(KERNEL_FCFS) $(OUTPUT_BIN)
	@echo "Created $(OUTPUT_BIN) - ready to flash"

disasm: build
	rust-objdump -d $(KERNEL_FCFS) | head -200

clean:
	cargo clean
	rm -f $(OUTPUT_BIN)
