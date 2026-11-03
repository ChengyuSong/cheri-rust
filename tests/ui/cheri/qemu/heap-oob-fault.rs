// Out-of-bounds read one element past a heap allocation (the allocation edge).
//  off:  software bounds check kept -> Rust panic (exit 101).
//  drop: -Zcheri-drop-bounds-checks -> no software check; the Vec's capability is bounded
//        to its 32-byte allocation by liballoc (the bump allocator itself hands out
//        1 MiB arena capabilities), so the load faults with a CHERI length violation.
// See README.md in this directory.
//@ revisions: off drop
//@ [off] run-fail
//@ [drop] run-crash
//@ [drop] compile-flags: -Zcheri-drop-bounds-checks
//@ check-run-results
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ aux-build: cheri_heap.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
#![no_std]
#![no_main]

extern crate alloc;
extern crate cheri_heap;
extern crate cheri_rt;

use alloc::vec::Vec;
use core::hint::black_box;

#[inline(never)]
fn get(a: &[u64], i: usize) -> u64 {
    a[i]
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    // Something after it in the arena, so a wide capability would read valid memory.
    let v: Vec<u64> = alloc::vec![1, 2, 3, 4];
    let after: Vec<u64> = alloc::vec![5; 64];
    black_box(&after);
    assert_eq!(get(black_box(&v), black_box(3)), 4);
    let x = get(black_box(&v), black_box(4));
    black_box(x);
}
