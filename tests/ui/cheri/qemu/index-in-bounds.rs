// In-bounds slice/array indexing with Rust's bounds checks runs to completion on
// QEMU-CHERI (riscv64 purecap, bare metal). See README.md in this directory.
//@ run-pass
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
//@ revisions: off drop elide
//@ [drop] compile-flags: -Zcheri-drop-bounds-checks
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
#![no_std]
#![no_main]

extern crate cheri_rt;

use core::hint::black_box;

#[inline(never)]
fn sum(a: &[u64], n: usize) -> u64 {
    let mut s = 0;
    for i in 0..n {
        s += a[i];
    }
    s
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let a = [1u64, 2, 3, 4, 5, 6, 7, 8];
    assert_eq!(sum(black_box(&a), black_box(8)), 36);
    let v: &[u64] = black_box(&a[2..6]);
    assert_eq!(v[black_box(3)], 6);
}
