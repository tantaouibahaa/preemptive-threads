# preemptive-threads: build, run and test targets.
#
#   make                 user programs -> initrd -> pt-os kernel (and the examples)
#   make test            thread tests, OS tests and shell sessions on every emulated machine
#   make test-threads    the thread test kernel (examples/test_kernel.rs) on every machine
#   make test-os         the OS test kernel (os/examples/os_test.rs) on every machine
#   make test-shell      a scripted shell session typed into pt-os's serial port on every machine
#   make test-virt       QEMU virt, entered at EL1 (PSCI over HVC)
#   make test-virt-el2   QEMU virt with virtualization=on (EL2 -> EL1 drop, PSCI over SMC)
#   make test-raspi3b    QEMU raspi3b booting a flat image through the spin-table stub at EL2
#   make test-os-<machine>  the OS test kernel on one machine
#   make test-shell-<machine>  the shell session on one machine
#   make user            build the EL0 programs packed into pt-os's initrd
#   make run-os-virt     interactive pt-os shell on QEMU virt (Ctrl-A X quits)
#   make run-os-raspi3b  interactive pt-os shell on QEMU raspi3b (Ctrl-A X quits)
#   make kernel8.img     demo kernel image for a Raspberry Pi Zero 2 W SD card
#   make build/pt-os-kernel8.img  pt-os image for a Raspberry Pi SD card
#   make build/os-test-pi.img     OS test kernel for a real Pi (no semihosting: prints the verdict, then halts)
#   make run-raspi3b     run the demo on QEMU raspi3b (Ctrl-C to quit)
#   make run-virt        run the demo on QEMU virt
#   make clippy          lint both board configurations
#   make test FEATURES=slow-uart  run the tests with a UART as slow as a real one at 115200 baud
#   make demo-gif        record a shell session on QEMU virt and render docs/demo.gif (needs Pillow)

CARGO        ?= cargo
JOBS         ?= 4
TARGET       := aarch64-unknown-none
TIMEOUT      ?= 180
OBJCOPY      ?= $(shell ls $$(rustc --print sysroot)/lib/rustlib/*/bin/rust-objcopy 2>/dev/null | head -1)

RPI_DIR      := target/rpi
VIRT_DIR     := target/virt
RPI_OUT      := $(RPI_DIR)/$(TARGET)/release/examples
VIRT_OUT     := $(VIRT_DIR)/$(TARGET)/release/examples
USER_DIR     := target/user
PASS_MARKER  := ALL [0-9]+ TESTS PASSED on 4 cores
SHELL_PASS   := SHELL SESSION PASSED
# Interactive serial console: Ctrl-C goes to the guest, Ctrl-A X quits QEMU.
CONSOLE      := -chardev stdio,id=con,mux=on,signal=off -serial chardev:con -mon chardev=con,mode=readline
OS_PASS      := ALL [0-9]+ OS TESTS PASSED on 4 cores
# Extra cargo features for the emulator test kernels and pt-os (e.g.
# FEATURES=slow-uart). Never applied to the SD card images.
FEATURES     ?=
comma        := ,
XF           := $(if $(FEATURES),$(comma)$(FEATURES))

.PHONY: all build user test test-threads test-os test-virt test-virt-el2 test-raspi3b \
        test-os-virt test-os-virt-el2 test-os-raspi3b test-shell test-shell-virt test-shell-virt-el2 \
        test-shell-raspi3b run-raspi3b run-virt run-os-virt run-os-raspi3b \
        clippy doc clean build/test-virt.elf build/test-raspi3b.img kernel8.img build/demo-virt.elf \
        build/os-test-virt.elf build/os-test-raspi3b.img build/pt-os-virt.elf build/pt-os-kernel8.img demo-gif build/os-test-pi.img

all: build build/pt-os-virt.elf build/pt-os-kernel8.img

build: user
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR) --examples
	$(CARGO) build -j$(JOBS) --release --target-dir $(VIRT_DIR) --examples --features qemu-virt
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR) -p pt-os --bins --examples
	$(CARGO) build -j$(JOBS) --release --target-dir $(VIRT_DIR) -p pt-os --bins --examples --features qemu-virt

# EL0 programs (user/bin: the userland, user/tests: kernel-test programs),
# packed into pt-os's initrd by os/build.rs.
user:
	$(CARGO) build -j$(JOBS) --release --target-dir $(USER_DIR) -p pt-userland -p pt-user-tests --bins

# --- test kernels ---------------------------------------------------------

build/test-virt.elf:
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(VIRT_DIR) --example test_kernel --features qemu-virt,semihosting$(XF)
	cp $(VIRT_OUT)/test_kernel $@

build/test-raspi3b.img:
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR) --example test_kernel --features semihosting$(XF)
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

test-threads: test-virt test-virt-el2 test-raspi3b

# --- OS test kernel ---------------------------------------------------------

build/os-test-virt.elf: user
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(VIRT_DIR) -p pt-os --example os_test --features qemu-virt,semihosting$(XF)
	cp $(VIRT_OUT)/os_test $@

build/os-test-raspi3b.img: user
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR) -p pt-os --example os_test --features semihosting$(XF)
	$(OBJCOPY) -O binary $(RPI_OUT)/os_test $@

test-os-virt: build/os-test-virt.elf
	scripts/qemu-run.sh virt $< $(TIMEOUT) build/os-test-virt.log
	grep -Eq "$(OS_PASS)" build/os-test-virt.log

test-os-virt-el2: build/os-test-virt.elf
	scripts/qemu-run.sh virt-el2 $< $(TIMEOUT) build/os-test-virt-el2.log
	grep -Eq "$(OS_PASS)" build/os-test-virt-el2.log

test-os-raspi3b: build/os-test-raspi3b.img
	scripts/qemu-run.sh raspi3b $< $(TIMEOUT) build/os-test-raspi3b.log
	grep -Eq "$(OS_PASS)" build/os-test-raspi3b.log

test-os: test-os-virt test-os-virt-el2 test-os-raspi3b

# For real hardware, where a semihosting call would trap. Own target dir,
# so it does not flip the features of the emulator build back and forth.
build/os-test-pi.img: user
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR)-hw -p pt-os --example os_test
	$(OBJCOPY) -O binary $(RPI_DIR)-hw/$(TARGET)/release/examples/os_test $@

# --- shell sessions (real PL011 receive path) -------------------------------

test-shell-virt: build/pt-os-virt.elf
	scripts/shell-session.py virt $< $(TIMEOUT) build/shell-virt.log
	grep -q "$(SHELL_PASS)" build/shell-virt.log

test-shell-virt-el2: build/pt-os-virt.elf
	scripts/shell-session.py virt-el2 $< $(TIMEOUT) build/shell-virt-el2.log
	grep -q "$(SHELL_PASS)" build/shell-virt-el2.log

test-shell-raspi3b: build/pt-os-kernel8.img
	scripts/shell-session.py raspi3b $< $(TIMEOUT) build/shell-raspi3b.log
	grep -q "$(SHELL_PASS)" build/shell-raspi3b.log

test-shell: test-shell-virt test-shell-virt-el2 test-shell-raspi3b

test: test-threads test-os test-shell

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

# --- pt-os kernel -----------------------------------------------------------

build/pt-os-virt.elf: user
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(VIRT_DIR) -p pt-os --bin pt-os --features qemu-virt$(XF)
	cp $(VIRT_DIR)/$(TARGET)/release/pt-os $@

build/pt-os-kernel8.img: user
	@mkdir -p build
	$(CARGO) build -j$(JOBS) --release --target-dir $(RPI_DIR) -p pt-os --bin pt-os $(if $(FEATURES),--features $(FEATURES))
	$(OBJCOPY) -O binary $(RPI_DIR)/$(TARGET)/release/pt-os $@

run-os-virt: build/pt-os-virt.elf
	qemu-system-aarch64 -M virt,gic-version=2 -cpu cortex-a53 -smp 4 -m 512M -display none $(CONSOLE) -kernel $<

run-os-raspi3b: build/pt-os-kernel8.img
	qemu-system-aarch64 -M raspi3b -display none $(CONSOLE) -kernel $<

# The README's recording: typed at human speed into the serial port, every
# step checked, saved as an asciicast and rendered to a GIF.
demo-gif: build/pt-os-virt.elf
	scripts/shell-session.py virt $< $(TIMEOUT) build/shell-record.log --record=build/shell-demo.cast
	scripts/cast2gif.py build/shell-demo.cast docs/demo.gif

# --- lint / docs ----------------------------------------------------------

clippy: user
	$(CARGO) clippy -j$(JOBS) --release --target-dir $(RPI_DIR) --examples --features semihosting,slow-uart -- -D warnings
	$(CARGO) clippy -j$(JOBS) --release --target-dir $(VIRT_DIR) --examples --features qemu-virt,semihosting -- -D warnings
	$(CARGO) clippy -j$(JOBS) --release --target-dir $(RPI_DIR) -p pt-os --bins --examples --features semihosting,slow-uart -- -D warnings
	$(CARGO) clippy -j$(JOBS) --release --target-dir $(VIRT_DIR) -p pt-os --bins --examples --features qemu-virt,semihosting -- -D warnings
	$(CARGO) clippy -j$(JOBS) --release --target-dir $(USER_DIR) -p ulib -- -D warnings
	$(CARGO) clippy -j$(JOBS) --release --target-dir $(USER_DIR) -p pt-userland -p pt-user-tests --bins -- -D warnings

doc: user
	$(CARGO) doc -j$(JOBS) --no-deps --target-dir $(RPI_DIR)
	$(CARGO) doc -j$(JOBS) --no-deps --target-dir $(RPI_DIR) -p pt-os
	$(CARGO) doc -j$(JOBS) --no-deps --target-dir $(USER_DIR) -p ulib

clean:
	$(CARGO) clean
	rm -f kernel8.img build/*.elf build/*.img build/*.log
