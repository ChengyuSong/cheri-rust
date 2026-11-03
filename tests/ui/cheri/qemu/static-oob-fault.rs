// Out-of-bounds read one element past a static array.
//  off:  software bounds check kept -> Rust panic (exit 101).
//  drop: -Zcheri-drop-bounds-checks -> no software check; the static's capability comes
//        from a __cap_relocs entry sized to the symbol (32 bytes), so the load faults.
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

static BEFORE: [u64; 8] = [9; 8];
static A: [u64; 4] = [1, 2, 3, 4];
static AFTER: [u64; 8] = [9; 8];

#[inline(never)]
fn get(a: &[u64], i: usize) -> u64 {
    a[i]
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    black_box(&BEFORE);
    black_box(&AFTER);
    assert_eq!(get(black_box(&A), black_box(3)), 4);
    black_box(get(black_box(&A), black_box(4)));
}
