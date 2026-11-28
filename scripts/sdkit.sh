#!/usr/bin/env bash
# Assemble the Raspberry Pi Zero 2 W SD card kit.
#
# usage: scripts/sdkit.sh <out dir> <hw target dir> <tools target dir> <user target dir>
#
# Expects the ELFs to be built already (see `make sdkit`). Converts each to a
# flat image, checks it, fetches the pinned firmware, writes config.txt,
# README.txt and SHA256SUMS.
#
# Every image is checked for
#   - semihosting calls (`hlt #0xf000`, 0xD45E0000): they trap on hardware;
#   - Crypto-extension instructions (AES, SHA, SM3/4, ...): the BCM2837
#     does not implement them.
# The user programs packed into pt-os's initrd are checked the same way.
#
# Firmware comes from raspberrypi/firmware tag $FW_TAG, checked against the
# SHA-256 sums below. A copy in $FW_LOCAL (default ~/Desktop/pt-sd-card) with
# the right sum is used instead of downloading.
set -euo pipefail

out=$1
hw=$2/aarch64-unknown-none/release
tools=$3/aarch64-unknown-none/release
user=$4/aarch64-unknown-none/release
here=$(cd "$(dirname "$0")/.." && pwd)

FW_TAG=${FW_TAG:-1.20260521}
FW_LOCAL=${FW_LOCAL:-$HOME/Desktop/pt-sd-card}
FW_CACHE=${FW_CACHE:-$here/build/firmware/$FW_TAG}
FIRMWARE=(
  "bootcode.bin e8b59fe10a4daa3cd2b127479f40b9856cfe3677154e8f24ae2aa70e3bb7c8eb"
  "start.elf 55049e2e35d6ddff3e5b93d6cface5cdc6df4e7ab1474af78d1af6ce8db4c9df"
  "fixup.dat b2d19b8c300b5a4ddbd0fcff3a0f7de61a171046269d8724e74f616058417d4b"
  "bcm2710-rpi-zero-2-w.dtb ce3ca3176658cddfad2ef6414d858bad61ce5242382eb0755248ea0edfcb0783"
  "overlays/disable-bt.dtbo ea69d22dedc607fee75eec57d8a4cc0f0eab93cd75393e61a64c49fbac912d02"
)

sysroot=$(rustc --print sysroot)
OBJCOPY=$(ls "$sysroot"/lib/rustlib/*/bin/rust-objcopy | head -1)
OBJDUMP=$(ls "$sysroot"/lib/rustlib/*/bin/llvm-objdump | head -1)

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

sum_of() { sha256 "$1" | cut -d' ' -f1; }

fail() { echo "sdkit: $*" >&2; exit 1; }

# check <elf> [img]: no crypto instructions and no semihosting in the code,
# and no semihosting word anywhere in the flat image.
check() {
  local elf=$1 img=${2:-}
  local bad
  bad=$("$OBJDUMP" -d --no-show-raw-insn "$elf" |
    grep -E $'\t(aese|aesd|aesmc|aesimc|sha1[chmp]|sha1su[01]|sha256h2?|sha256su[01]|sha512[a-z0-9]*|sm3[a-z0-9]*|sm4[a-z0-9]*|eor3|bcax|rax1|xar|hlt)\t|\tpmull2?\t.*\\.1q' || true)
  [ -z "$bad" ] || fail "$elf contains forbidden instructions:"$'\n'"$bad"
  if [ -n "$img" ]; then
    python3 -I -c '
import struct, sys
data = open(sys.argv[1], "rb").read()
data += b"\0" * (-len(data) % 4)
hits = [i * 4 for i, (w,) in enumerate(struct.iter_unpack("<I", data)) if w == 0xD45E0000]
if hits:
    sys.exit("%s: hlt #0xf000 at offsets %s" % (sys.argv[1], ", ".join(hex(h) for h in hits)))
' "$img" || fail "semihosting call in $img"
  fi
}

mkdir -p "$out/overlays" "$FW_CACHE/overlays"

# --- firmware ---------------------------------------------------------------
for entry in "${FIRMWARE[@]}"; do
  read -r f want <<<"$entry"
  cached=$FW_CACHE/$f
  if [ ! -f "$cached" ] || [ "$(sum_of "$cached")" != "$want" ]; then
    if [ -f "$FW_LOCAL/$f" ] && [ "$(sum_of "$FW_LOCAL/$f")" = "$want" ]; then
      /bin/cp -f "$FW_LOCAL/$f" "$cached"
    else
      echo "sdkit: fetching $f from raspberrypi/firmware $FW_TAG"
      curl -sSfL -o "$cached.part" "https://raw.githubusercontent.com/raspberrypi/firmware/$FW_TAG/boot/$f"
      mv "$cached.part" "$cached"
    fi
  fi
  [ "$(sum_of "$cached")" = "$want" ] || fail "$f: SHA-256 mismatch (want $want)"
  /bin/cp -f "$cached" "$out/$f"
done

# --- images -----------------------------------------------------------------
images=(
  "uart0.img $tools/uart0"
  "test8.img $hw/examples/test_kernel"
  "ostest8.img $hw/examples/os_test"
  "ptos8.img $hw/pt-os"
  "kernel8.img $hw/examples/rpi_kernel"
)
for entry in "${images[@]}"; do
  read -r img elf <<<"$entry"
  [ -f "$elf" ] || fail "missing $elf (run make sdkit)"
  "$OBJCOPY" -O binary "$elf" "$out/$img"
  check "$elf" "$out/$img"
done
nuser=0
for src in "$here"/user/bin/src/bin/*.rs "$here"/user/tests/src/bin/*.rs; do
  elf=$user/$(basename "$src" .rs)
  [ -f "$elf" ] || fail "missing user program $elf (run make user)"
  check "$elf"
  nuser=$((nuser + 1))
done

/bin/cp -f "$here/sdkit/config.txt" "$here/sdkit/README.txt" "$out/"
kit=(config.txt README.txt)
for entry in "${FIRMWARE[@]}" "${images[@]}"; do
  read -r f _ <<<"$entry"
  kit+=("$f")
done
printf '%s\n' "${kit[@]}" | sort > "$out/.files"
(cd "$out" && sha256 $(cat .files) > SHA256SUMS)

echo "sdkit: $out ready (firmware $FW_TAG; ${#images[@]} images and $nuser user programs checked:"
echo "       no semihosting calls, no Crypto-extension instructions)"
(cd "$out" && ls -l $(cat .files) SHA256SUMS | awk '{printf "  %9d  %s\n", $5, $9}')
