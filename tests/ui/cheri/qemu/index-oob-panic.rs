// Out-of-bounds slice indexing.
//  off:  bounds checks ON (no -Z cheri flags) -> Rust's software check panics (exit 101).
//  drop: -Zcheri-drop-bounds-checks -> no software check; the load goes through the
//        capability bounded to the 4-element stack array -> CHERI length fault.
//  elide: -Zcheri-cap-bounds-checks=elide (M1) -> 32-byte slice, exactly representable,
//        so the fast path drops the software check and the access through the exactly
//        bounded capability takes a CHERI length fault (not a panic).
// See README.md in this directory.
//@ revisions: off drop elide
//@ [off] run-fail
//@ [drop] run-crash
//@ [drop] compile-flags: -Zcheri-drop-bounds-checks
//@ [elide] run-crash
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ check-run-results
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
#![no_std]
#![no_main]

extern crate cheri_rt;

use core::hint::black_box;

#[inline(never)]
fn get(a: &[u64], i: usize) -> u64 {
    a[i]
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let a = [1u64, 2, 3, 4];
    let x = get(black_box(&a), black_box(8));
    black_box(x);
}
