# Preemptive Threads - Raspberry Pi Zero 2 W
# Makefile for building and running the kernel

TARGET := aarch64-unknown-none
KERNEL := target/$(TARGET)/release/examples/rpi_kernel
KERNEL_BIN := kernel8.img

.PHONY: build build-virt run run-virt debug debug-virt clean test test-qemu test-virt binary

#=============================================================================
# Default: Build for real Pi / QEMU raspi3b (no preemption in QEMU)
#=============================================================================

build:
	cargo +nightly build --release --example rpi_kernel --target $(TARGET)

# QEMU raspi3b - boots and runs but no preemption (GIC not emulated)
run: build
	qemu-system-aarch64 \
		-M raspi3b \
		-kernel $(KERNEL) \
		-serial stdio \
		-display none

#=============================================================================
# QEMU virt machine - full preemption testing (GIC works)
#=============================================================================

build-virt:
	RUSTFLAGS="-C link-arg=-Tqemu_virt.ld" cargo +nightly build --release --example rpi_kernel --target $(TARGET) --features qemu-virt

# QEMU virt - full preemption works (GIC emulated at 0x08000000)
run-virt: build-virt
	qemu-system-aarch64 \
		-M virt \
		-cpu cortex-a53 \
		-kernel $(KERNEL) \
		-serial stdio \
		-display none

#=============================================================================
# Debug targets
#=============================================================================

debug: build
	qemu-system-aarch64 \
		-M raspi3b \
		-kernel $(KERNEL) \
		-serial stdio \
		-display none \
		-d int,cpu_reset

debug-virt: build-virt
	qemu-system-aarch64 \
		-M virt \
		-cpu cortex-a53 \
		-kernel $(KERNEL) \
		-serial stdio \
		-display none \
		-d int,cpu_reset

# GDB server (for step-through debugging)
gdb: build
	qemu-system-aarch64 \
		-M raspi3b \
		-kernel $(KERNEL) \
		-serial stdio \
		-display none \
		-S -s

#=============================================================================
# Real hardware
#=============================================================================

# Create raw binary for real Pi SD card
binary: build
	rust-objcopy -O binary $(KERNEL) $(KERNEL_BIN)
	@echo "Created $(KERNEL_BIN) - copy to SD card"

#=============================================================================
# Testing
#=============================================================================

test:
	cargo test --features std-shim

# Quick QEMU test (5 seconds) - raspi3b
test-qemu: build
	@rm -f /tmp/qemu_output.txt /tmp/qemu_debug.txt
	@echo "Running raspi3b for 5 seconds..."
	@qemu-system-aarch64 -M raspi3b -kernel $(KERNEL) \
		-serial file:/tmp/qemu_output.txt -display none \
		-d cpu_reset,int 2>/tmp/qemu_debug.txt & \
	QEMU_PID=$$!; sleep 5; kill $$QEMU_PID 2>/dev/null; wait $$QEMU_PID 2>/dev/null; \
	echo "=== Serial Output ==="; cat /tmp/qemu_output.txt 2>/dev/null || echo "(no output)"; \
	echo "=== QEMU Debug (last 20 lines) ==="; tail -20 /tmp/qemu_debug.txt 2>/dev/null

# Quick QEMU test (5 seconds) - virt with preemption
test-virt: build-virt
	@rm -f /tmp/qemu_output.txt /tmp/qemu_debug.txt
	@echo "Running virt for 5 seconds..."
	@qemu-system-aarch64 -M virt -cpu cortex-a53 -kernel $(KERNEL) \
		-serial file:/tmp/qemu_output.txt -display none \
		-d cpu_reset,int 2>/tmp/qemu_debug.txt & \
	QEMU_PID=$$!; sleep 5; kill $$QEMU_PID 2>/dev/null; wait $$QEMU_PID 2>/dev/null; \
	echo "=== Serial Output ==="; cat /tmp/qemu_output.txt 2>/dev/null || echo "(no output)"; \
	echo "=== QEMU Debug (last 20 lines) ==="; tail -20 /tmp/qemu_debug.txt 2>/dev/null

#=============================================================================
# Utilities
#=============================================================================

clean:
	cargo clean
	rm -f $(KERNEL_BIN)

disasm: build
	rust-objdump -d $(KERNEL) | head -200
