// M3.0 export-trust fix, at run time: a non-Rust-ABI function (callable from C) narrows its
// safe-pointer parameters to the referent at entry, so a caller-supplied capability that is
// wider than the referent (here: the whole 512-byte static) is not trusted inside it.
// Rust-ABI functions are unchanged under a genuinely wide param. `nofix`
// (-Zcheri-export-entry-bounds=no -Zcheri-contract-narrow=no) is the M2 behaviour:
// construction stays wide and the wide capability reaches the body.
//@ run-pass
//@ revisions: fix nofix
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
//@ [nofix] compile-flags: -Zcheri-export-entry-bounds=no -Zcheri-contract-narrow=no
#![no_std]
#![no_main]
#![feature(stdarch_cheri)]
#![feature(core_intrinsics)]
#![allow(improper_ctypes_definitions)]
// M3.6: transmute raw→typed is a conversion; allow the undischarged lint here so
// the test can still construct a deliberately wide capability for the entry-bounds
// experiment. `#nofix` also turns off conversion-narrow so the wide cap reaches
// the callee (otherwise transmute itself would establish the referent).
#![allow(mem_contract_undischarged)]

extern crate cheri_rt;

use core::arch::cheri;
use core::hint::black_box;

static BIG: [u64; 64] = [7; 64]; // capability bounds: 512 bytes

#[inline(never)]
extern "C" fn c_ref(a: &[u64; 4]) -> (usize, u64) {
    (cheri::length_get(a.as_ptr()), a[3])
}
#[inline(never)]
extern "C" fn c_slice(s: &[u64]) -> (usize, u64) {
    (cheri::length_get(s.as_ptr()), s[s.len() - 1])
}
#[inline(never)]
extern "C" fn c_mut(x: &mut u64) -> usize {
    *x += 1;
    cheri::length_get(x as *const u64)
}
#[inline(never)]
fn rust_ref(a: &[u64; 4]) -> usize {
    cheri::length_get(a.as_ptr())
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let wide: *const u64 = black_box(BIG.as_ptr());
    assert_eq!(cheri::length_get(wide), 512);
    // Deliberately wide refs into BIG. M3.6 treats transmute raw→typed as a
    // conversion (lint + narrow); this file allows the lint, and `#nofix` also
    // passes `-Zcheri-contract-narrow=no` so construction stays wide and the
    // entry-bounds-off path is actually observable.
    let r: &[u64; 4] = unsafe { core::mem::transmute(wide) };
    let raw: *const [u64] =
        core::intrinsics::aggregate_raw_ptr::<*const [u64], _, _>(wide, 8);
    let s: &[u64] = unsafe { core::mem::transmute(raw) };
    let f_ref: extern "C" fn(&[u64; 4]) -> (usize, u64) = black_box(c_ref);
    let f_slice: extern "C" fn(&[u64]) -> (usize, u64) = black_box(c_slice);
    let f_rust: fn(&[u64; 4]) -> usize = black_box(rust_ref);
    let (lr, vr) = f_ref(r);
    let (ls, vs) = f_slice(s);
    assert_eq!((vr, vs), (7, 7)); // values unchanged by the narrowing
    let mut m = [0u64; 8]; // escaping stack array: 64-byte capability
    // Same allow + `#nofix` contract-narrow=no: keep stack-array capability wide.
    let mp: &mut u64 = unsafe { core::mem::transmute(black_box(m.as_mut_ptr())) };
    let lm = black_box(c_mut as extern "C" fn(&mut u64) -> usize)(mp);
    assert_eq!(m[0], 1);
    let lrust = f_rust(r);
    // #fix: transmute is a conversion-narrow (M3.6), so `r`/`s`/`mp` are already
    // referent-sized at construction; C entry-narrow is then a no-op, and Rust-ABI
    // `rust_ref` correctly sees the established 32 B (not 512). The "Rust ABI does
    // not entry-narrow a wide param" check lives in `#nofix`, where
    // `-Zcheri-contract-narrow=no` keeps construction genuinely wide.
    #[cfg(fix)]
    assert_eq!((lr, ls, lm, lrust), (32, 64, 8, 32));
    #[cfg(nofix)]
    assert_eq!((lr, ls, lm, lrust), (512, 512, 64, 512));
}
