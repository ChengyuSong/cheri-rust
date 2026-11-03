# tests/ui/cheri/qemu: run tests on QEMU-CHERI (riscv64cheri-unknown-none-purecap)

LLM-generated research prototype (local branch `local/m1-qemu-runner`); not for upstream.

These are `run-*` UI tests that are actually executed on bare-metal QEMU-CHERI
(`-M spike`, HTIF) via `src/etc/cheri-qemu-runner.sh`. They only run for
`--target riscv64cheri-unknown-none-purecap` (`//@ only-...`).

## Setup (bootstrap.toml, local)

    [target.riscv64cheri-unknown-none-purecap]
    runner = "/abs/path/src/etc/cheri-qemu-runner.sh --qemu /abs/path/qemu-system-riscv64xcheri"

Run:

    ./x test --stage 1 tests/ui/cheri/qemu --target riscv64cheri-unknown-none-purecap

## Writing a test

    #![no_std]
    #![no_main]
    extern crate cheri_rt;                       // entry, trap handler, panic handler, HTIF exit
    #[unsafe(no_mangle)]
    extern "C" fn cheri_test_main() { ... }      // returning = exit 0

with directives

    //@ only-riscv64cheri-unknown-none-purecap
    //@ aux-build: cheri_rt.rs
    //@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld

`cheri_rt::println!` writes to the HTIF console (QEMU stdout -> `.run.stdout`);
`cheri_rt::exit(code)` exits with a code.

## Outcomes and exit codes

| outcome                        | runner exit      | compiletest | stderr line (from runner)                                   |
|--------------------------------|------------------|-------------|-------------------------------------------------------------|
| `cheri_test_main` returns      | 0                | Pass        | `cheri-runner: exit 0`                                      |
| Rust panic (`#[panic_handler]`)| 101              | Fail        | `cheri-runner: panic (exit 101)`                            |
| CHERI fault (mcause 28)        | 128 + cause      | Crash       | `cheri-runner: CHERI fault: length violation (cause 0x01)`  |
| other trap                     | 192 + mcause     | Crash       | `cheri-runner: trap mcause 2` (e.g. `-Cpanic=immediate-abort`) |
| timeout (`CHERI_QEMU_TIMEOUT`, default 60 s) | 124 | Fail     | `cheri-runner: TIMEOUT after 60s`                           |

The panic handler also prints `panicked at FILE:L:C:` and the message (on its own
line) to `.run.stdout`.

## Declaring expected panic vs expected CHERI fault

compiletest alone cannot tell them apart: this target is `panic=abort`, so
`run-fail` also accepts a Crash. Always add `//@ check-run-results` and pin the
runner's status line (run output is compared as a *line subset* when a runner
is configured, so only the listed lines must appear):

* **expected panic** (software bounds check fires):

      //@ run-fail
      //@ check-run-results
      # foo.run.stderr:  cheri-runner: panic (exit 101)
      # foo.run.stdout:  index out of bounds: the len is 4 but the index is 8   (optional)

* **expected CHERI fault** (hardware capability check fires):

      //@ run-crash
      //@ check-run-results
      # foo.run.stderr:  cheri-runner: CHERI fault: length violation (cause 0x01)

* per flag, with revisions (as used by `index-oob-panic.rs` and `elide-smoke.rs`):

      //@ revisions: off elide
      //@ [off] run-fail
      //@ [elide] run-crash
      //@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
      # foo.off.run.stderr:   cheri-runner: panic (exit 101)
      # foo.elide.run.stderr: cheri-runner: CHERI fault: length violation (cause 0x01)

  Per-revision `run-fail`/`run-crash` works here (`//@ [rev] run-crash`). Per-revision
  parameters can be passed with `//@ [rev] rustc-env:NAME=value` + `env!` (see elide-smoke.rs).

Under `-Zcheri-cap-bounds-checks=elide` (M1), an out-of-bounds `s[i]` gives a **CHERI fault**
only when `csetboundsexact(s, len)` is exact (fast path, check removed); when the length is not
exactly representable at that base, the tag is cleared and the slow path keeps the software
check, giving a **panic** (same as `off`).

## Tests

* `index-in-bounds.rs` (revisions off, drop, elide): run-pass.
* `index-oob-panic.rs`: revisions `off` (run-fail, panic line + index message), `drop`
  (`-Zcheri-drop-bounds-checks`, run-crash, CHERI length violation) and `elide`
  (`-Zcheri-cap-bounds-checks=elide`, 32-byte exact slice: run-crash, CHERI length violation).
* `elide-smoke.rs`: port of the M1 QEMU smoke matrix, `s = &BUF[off..off+len]`, `s[len]`:
  exact16 / exact4096 @0 -> off panic, elide CHERI fault; inexact4096 @1 / unrep4097 @1 ->
  panic in both modes (elide slow path).
* `elide-smoke-inbounds.rs` (off, elide): every smoke (len, off) incl. 8192 @0 and 5 @3,
  first/middle/last index, load + store; run-pass.
* `raw-oob-cheri-fault.rs`: run-crash, expects a CHERI length violation (raw pointer
  past a 4-element stack array). It is the template for the M1 `elide` expectation.

### Allocation-level bounds (M1r)

These need no `-Z cheri-cap-bounds-checks` mode: the software checks stay, and the capability
of every heap block, escaping stack array and static is bounded to its allocation, padded to
an exactly representable length. `-Zcheri-drop-bounds-checks` is used only to reach the
allocation edge without the software check.

* `auxiliary/cheri_heap.rs`: bump `#[global_allocator]` over a 1 MiB static arena. It hands
  out arena-wide capabilities; liballoc's `alloc`/`alloc_zeroed`/`realloc` pad (CRRL), align
  (CRAM of the padded length) and `CSetBoundsExact` each block. Not a ZST: ld.lld warns
  "could not determine size of cap reloc" for a capability to a zero-sized static.
* `heap-oob-fault.rs`, `stack-oob-fault.rs`, `static-oob-fault.rs` (off, drop): `a[len]` one
  element past a 32-byte `Vec` / escaping stack array / static, with live neighbours.
  off -> panic + message; drop -> CHERI length violation.
* `alloc-bounds.rs`: run-pass; base == address, length == CRRL(size), address aligned to
  CRAM for Box, `vec![0; n]` and `Vec::with_capacity(n)` (n up to 70000, incl. 0x1ff9,
  0x3ff1, 0x3ff8), 3000 `push`es, `shrink_to_fit`, `into_boxed_slice`, `String`, `Rc`, stack
  arrays and statics of 32, 5000, 0x1ff9 and 0x3ff8 bytes. The 0x3ff8 stack case fails
  without the LLVM representability fix (base 16 bytes off, bounds 0x4020).
* `bounded-alloc-wrapper.rs` (root, noroot): `alloc::alloc::CheriBoundedAlloc` around an
  allocator that keeps a header in front of each block. noroot: the bounded capability
  reaches `dealloc` and the header read faults; root: `with_root` re-derives from the heap
  root and the program completes.
* `auxiliary/cheri_heap_grow.rs`: bump allocator that resizes the most recent block in place
  (counters `REALLOCS`, `IN_PLACE`). `Grow::wide()` returns arena-derived capabilities;
  `Grow::narrow()` returns the pointer it was given when it resizes in place.
* `realloc-bounds.rs` (pass, drop_padding, drop_shrink), liballoc over `Grow::wide()`:
  growth inside the padding (4097 -> 4100, padded 4104) keeps the pointer and makes no
  allocator call; growth past the padding in place (same address, exact 6000) and by copy
  (blocked, new address, exact); growth needing stricter alignment (4000 -> 8100, allocate +
  copy); same-alignment shrink (6000 -> 4100, same address, bounds 4104); padding-only shrink.
  drop_padding: a read at 4103 works, 4104 faults. drop_shrink: after the shrink, 4104 faults.
* `realloc-wrapper.rs` (root, noroot): `CheriBoundedAlloc<Grow::narrow()>`. root: the grown
  block is re-derived from the heap root and stays in place; noroot: only the narrowed
  capability comes back, so liballoc moves the block. Both exact.
