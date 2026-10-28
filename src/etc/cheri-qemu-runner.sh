#!/usr/bin/env bash
# compiletest `runner` for riscv64cheri-unknown-none-purecap: boots a bare-metal test
# ELF on QEMU-CHERI (-M spike, HTIF) and maps the HTIF exit code to compiletest's
# Pass / Fail / Crash buckets. LLM-generated research prototype (not for upstream).
#
# bootstrap.toml:
#   [target.riscv64cheri-unknown-none-purecap]
#   runner = "/path/to/src/etc/cheri-qemu-runner.sh --qemu /path/to/qemu-system-riscv64xcheri"
# (compiletest splits `runner` on spaces and appends the test executable + run-flags.)
#
# Exit codes written by tests/ui/cheri/qemu/auxiliary/cheri_rt.rs:
#   0          normal return from cheri_test_main          -> exit 0    (Pass)
#   101        #[panic_handler]                            -> exit 101  (Fail)
#   128+cause  CHERI fault, mcause 28, cause = mtval[4:0]  -> exit 128+cause (Crash)
#   192+mcause any other trap (e.g. 194 = illegal insn)    -> exit 192+mcause (Crash)
#   timeout                                                -> exit 124  (Fail)
# A one-line status "cheri-runner: ..." is printed to stderr; tests match it with
# `//@ check-run-results` + a .run.stderr file (compared as a line subset).
set -u
QEMU=${CHERI_QEMU:-qemu-system-riscv64xcheri}
TIMEOUT=${CHERI_QEMU_TIMEOUT:-60}
while [ $# -gt 0 ]; do
  case "$1" in
    --qemu) QEMU=$2; shift 2 ;;
    --timeout) TIMEOUT=$2; shift 2 ;;
    *) break ;;
  esac
done
[ $# -ge 1 ] || { echo "usage: $0 [--qemu PATH] [--timeout S] test.elf [args...]" >&2; exit 2; }
ELF=$1; shift
[ $# -eq 0 ] || echo "cheri-runner: warning: ignoring run-flags: $*" >&2
timeout --foreground -k 5 "$TIMEOUT" "$QEMU" -M spike -nographic -bios none \
  -kernel "$ELF" </dev/null
rc=$?
cheri_cause() {
  case $1 in
    1) echo "length violation" ;;   2) echo "tag violation" ;;
    3) echo "seal violation" ;;     4) echo "type violation" ;;
    8) echo "software-defined permission violation" ;;
    10) echo "representability violation" ;; 11) echo "unaligned base" ;;
    16) echo "global violation" ;;  17) echo "permit execute violation" ;;
    18) echo "permit load violation" ;; 19) echo "permit store violation" ;;
    20) echo "permit load capability violation" ;; 21) echo "permit store capability violation" ;;
    22) echo "permit store local capability violation" ;;
    *) echo "cause $1" ;;
  esac
}
if [ $rc -eq 0 ]; then
  echo "cheri-runner: exit 0" >&2; exit 0
elif [ $rc -eq 124 ] || [ $rc -eq 137 ]; then
  echo "cheri-runner: TIMEOUT after ${TIMEOUT}s" >&2; exit 124
elif [ $rc -eq 101 ]; then
  echo "cheri-runner: panic (exit 101)" >&2; exit 101
elif [ $rc -ge 128 ] && [ $rc -lt 160 ]; then
  c=$((rc - 128))
  printf 'cheri-runner: CHERI fault: %s (cause 0x%02x)\n' "$(cheri_cause $c)" $c >&2; exit $rc
elif [ $rc -ge 192 ] && [ $rc -lt 256 ]; then
  echo "cheri-runner: trap mcause $((rc - 192))" >&2; exit $rc
else
  echo "cheri-runner: exit $rc" >&2; exit $rc
fi
