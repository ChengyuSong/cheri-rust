// Expected-fault template: an out-of-bounds load through a capability bounded to
// a 4-element stack array traps with a CHERI length violation (mcause 28,
// cause 0x01 -> runner exit 129), with no Rust bounds check involved.
// See README.md in this directory.
//@ run-crash
//@ check-run-results
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
#![no_std]
#![no_main]

extern crate cheri_rt;

use core::hint::black_box;

#[inline(never)]
fn get_raw(a: *const u64, i: usize) -> u64 {
    unsafe { a.add(i).read_volatile() }
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let a = [1u64, 2, 3, 4];
    let x = get_raw(black_box(a.as_ptr()), black_box(8));
    black_box(x);
    // Not reached: the load above faults.
    cheri_rt::exit(0)
}
