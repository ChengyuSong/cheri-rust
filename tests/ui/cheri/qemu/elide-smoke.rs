// Port of the M1 QEMU smoke matrix (/workspace/cheri/m1-compiler/qemu-smoke): index a
// sub-slice s = &BUF[off..off+len] (BUF 8 KiB, 4096-aligned) at idx = len (one past the end).
// Under -Zcheri-cap-bounds-checks=elide, `s[idx]` is narrowed with csetboundsexact(s, len):
//   exact (len 16 @0, len 4096 @0)  -> fast path, no software check -> CHERI length fault
//   inexact (len 4096 @1), unrepresentable (len 4097 @1) -> tag cleared -> slow path keeps
//   the software check -> Rust panic, same as `off`.
// See README.md in this directory. LLM-generated research prototype (not for upstream).
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
//@ check-run-results
//@ revisions: off_exact16 elide_exact16 off_exact4096 elide_exact4096 off_inexact4096 elide_inexact4096 off_unrep4097 elide_unrep4097
//@ [off_exact16] run-fail
//@ [off_exact16] rustc-env:SMOKE_LEN=16
//@ [off_exact16] rustc-env:SMOKE_OFF=0
//@ [elide_exact16] run-crash
//@ [elide_exact16] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ [elide_exact16] rustc-env:SMOKE_LEN=16
//@ [elide_exact16] rustc-env:SMOKE_OFF=0
//@ [off_exact4096] run-fail
//@ [off_exact4096] rustc-env:SMOKE_LEN=4096
//@ [off_exact4096] rustc-env:SMOKE_OFF=0
//@ [elide_exact4096] run-crash
//@ [elide_exact4096] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ [elide_exact4096] rustc-env:SMOKE_LEN=4096
//@ [elide_exact4096] rustc-env:SMOKE_OFF=0
//@ [off_inexact4096] run-fail
//@ [off_inexact4096] rustc-env:SMOKE_LEN=4096
//@ [off_inexact4096] rustc-env:SMOKE_OFF=1
//@ [elide_inexact4096] run-fail
//@ [elide_inexact4096] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ [elide_inexact4096] rustc-env:SMOKE_LEN=4096
//@ [elide_inexact4096] rustc-env:SMOKE_OFF=1
//@ [off_unrep4097] run-fail
//@ [off_unrep4097] rustc-env:SMOKE_LEN=4097
//@ [off_unrep4097] rustc-env:SMOKE_OFF=1
//@ [elide_unrep4097] run-fail
//@ [elide_unrep4097] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ [elide_unrep4097] rustc-env:SMOKE_LEN=4097
//@ [elide_unrep4097] rustc-env:SMOKE_OFF=1
#![no_std]
#![no_main]

extern crate cheri_rt;

use core::hint::black_box;

#[repr(C, align(4096))]
struct Buf([u8; 8192]);
static BUF: Buf = Buf([7; 8192]);

const fn parse(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    let mut v = 0;
    while i < b.len() {
        v = v * 10 + (b[i] - b'0') as usize;
        i += 1;
    }
    v
}
const LEN: usize = parse(env!("SMOKE_LEN"));
const OFF: usize = parse(env!("SMOKE_OFF"));

#[inline(never)]
fn get(s: &[u8], i: usize) -> u8 {
    s[i]
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let (len, off) = (black_box(LEN), black_box(OFF));
    let s = &BUF.0[off..off + len];
    // In bounds first (both modes, every case), then one past the end.
    assert_eq!(get(s, black_box(len - 1)), 7);
    let x = get(s, black_box(len));
    black_box(x);
    // Not reached: the OOB index above panics or faults.
    cheri_rt::exit(0)
}
