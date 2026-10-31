// Out-of-bounds read one element past a stack array whose address escapes.
//  off:  software bounds check kept -> Rust panic (exit 101).
//  drop: -Zcheri-drop-bounds-checks -> no software check; CHERI LLVM's CheriBoundAllocas
//        bounded the escaping alloca to its 32 bytes, so the load faults.
// See README.md in this directory.
//@ revisions: off drop
//@ [off] run-fail
//@ [drop] run-crash
//@ [drop] compile-flags: -Zcheri-drop-bounds-checks
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

#[inline(never)]
fn run(i: usize) -> u64 {
    // A neighbour in the same frame, so a frame-wide capability would read valid memory.
    let before = [9u64; 8];
    let a = [1u64, 2, 3, 4];
    let after = [9u64; 8];
    black_box(&before);
    black_box(&after);
    get(black_box(&a), i)
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    assert_eq!(run(black_box(3)), 4);
    black_box(run(black_box(4)));
}
