#!/usr/bin/env bash
# Run a kernel under QEMU with a hard timeout and report its exit status.
#
# usage: scripts/qemu-run.sh <virt|virt-el2|raspi3b> <kernel> [timeout_s] [log]
#
# The test kernel ends with a semihosting SYS_EXIT, so QEMU's exit status is
# the kernel's verdict (0 = pass). A hang is killed by the watchdog after the
# timeout and reported as exit status 124. Serial output is shown and also
# written to the log file.
set -u
machine=$1
kernel=$2
timeout_s=${3:-120}
log=${4:-/dev/null}

case "$machine" in
  virt)     args=(-M virt,gic-version=2 -cpu cortex-a53 -smp 4 -m 512M) ;;
  virt-el2) args=(-M virt,gic-version=2,virtualization=on -cpu cortex-a53 -smp 4 -m 512M) ;;
  raspi3b)  args=(-M raspi3b -cpu cortex-a53) ;;  # the BCM2837 always has A53s; -cpu only documents it
  *) echo "unknown machine: $machine" >&2; exit 2 ;;
esac

out=$(mktemp)
trap 'rm -f "$out" "$out.timeout"' EXIT

qemu-system-aarch64 "${args[@]}" -display none -monitor none -serial stdio \
  -semihosting-config enable=on,target=native -kernel "$kernel" </dev/null >"$out" 2>&1 &
qemu=$!

# Watchdog: SIGTERM after the timeout, SIGKILL if QEMU still lingers.
(
  sleep "$timeout_s"
  if kill -0 "$qemu" 2>/dev/null; then
    touch "$out.timeout"
    kill -TERM "$qemu" 2>/dev/null
    sleep 5
    kill -KILL "$qemu" 2>/dev/null
  fi
) >/dev/null 2>&1 &
watchdog=$!

wait "$qemu"
status=$?
kill "$watchdog" 2>/dev/null
wait "$watchdog" 2>/dev/null

cat "$out"
cp "$out" "$log" 2>/dev/null || true
if [ -e "$out.timeout" ]; then
  echo "TIMEOUT: QEMU killed after ${timeout_s}s" | tee -a "$log"
  exit 124
fi
exit "$status"
