TARGET := aarch64-unknown-none
KERNEL := target/$(TARGET)/release/examples/fcfs_kernel
KERNEL_RPI := target/$(TARGET)/release/examples/rpi_kernel
BIN := kernel8.img

QEMU_PI   := qemu-system-aarch64 -M raspi3b -kernel $(KERNEL) -serial stdio -display none
QEMU_VIRT := qemu-system-aarch64 -M virt,gic-version=2 -cpu cortex-a53 -kernel $(KERNEL) -serial stdio -display none

.PHONY: build build-rpi build-virt run run-rpi run-virt debug debug-virt gdb binary test clean disasm


build:
	cargo +nightly build --release --example fcfs_kernel --target $(TARGET)

build-rpi:
	cargo +nightly build --release --example rpi_kernel --target $(TARGET)

build-virt:
	RUSTFLAGS="-C link-arg=-Tqemu_virt.ld" \
	cargo +nightly build --release --example fcfs_kernel --target $(TARGET) --features qemu-virt

run: build
	$(QEMU_PI)

run-rpi: build-rpi
	qemu-system-aarch64 -M raspi3b -kernel $(KERNEL_RPI) -serial stdio -display none

run-virt: build-virt
	$(QEMU_VIRT)



debug: build
	$(QEMU_PI) -d int,cpu_reset

debug-virt: build-virt
	$(QEMU_VIRT) -d int,cpu_reset

gdb: build
	$(QEMU_PI) -S -s


binary: build
	rust-objcopy -O binary $(KERNEL) $(BIN)
	@echo "Created $(BIN)"



disasm: build
	rust-objdump -d $(KERNEL) | head -200

clean:
	cargo clean
	rm -f $(BIN)
