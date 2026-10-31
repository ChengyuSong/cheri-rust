// M2.3: slices minted inside liballoc from `Vec<T, Global>` (and `String`, and reborrowed
// `Box<[T], Global>`) are trusted: liballoc's `Global` path bounds the capability to the
// allocation and `len <= cap`. In `narrow` mode their accesses are narrowed with
// `!mem.alloc_inbounds` + `mem.checked`, so the default skip pass removes the narrow, and
// the stronger no-skip mode narrows to `len` (spare capacity excluded). User-built raw
// slices (`from_raw_parts`) get a M3.3 contract_narrow (not discharged) and custom
// allocators are never trusted.
//
//@ revisions: noskip skip untrusted
//@ compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=narrow
// M5r R3 (D23): CHECKs below see the late CHERI pass decisions [LLM-generated].
//@ compile-flags: -Cllvm-args=-cheri-bounds-lowering-in-opt
//@ [noskip] compile-flags: -Cllvm-args=-cheri-skip-narrow=false
//@ [untrusted] compile-flags: -Cllvm-args=-cheri-skip-narrow=false -Zcheri-alloc-trusted-slices=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

// CHECK-LABEL: @vec_index(
#[no_mangle]
pub fn vec_index(v: &Vec<u64>, i: usize) -> u64 {
    // noskip: @llvm.cheri.cap.bounds.set.i64({{.*}}!cheri.access_narrow
    // noskip: panic_bounds_check
    // skip-NOT: llvm.cheri.cap.bounds.set
    // skip: panic_bounds_check
    // untrusted-NOT: llvm.cheri.cap.bounds.set
    // untrusted-NOT: !mem.alloc_inbounds
    // untrusted: getelementptr inbounds
    // untrusted: panic_bounds_check
    v[i]
}

// CHECK-LABEL: @vec_index_mut(
#[no_mangle]
pub fn vec_index_mut(v: &mut Vec<u64>, i: usize) {
    // M5r R3 (D23): the DerefMut slice claim narrows first; the check narrow of the same
    // bytes is then not repeated.
    // noskip: @llvm.cheri.cap.bounds.set.i64({{.*}}!cheri.{{(access|contract)}}_narrow
    // skip-NOT: llvm.cheri.cap.bounds.set
    // untrusted-NOT: !mem.alloc_inbounds
    // CHECK: ret void
    v[i] += 1
}

// CHECK-LABEL: @string_byte(
#[no_mangle]
pub fn string_byte(s: &String, i: usize) -> u8 {
    // noskip: @llvm.cheri.cap.bounds.set.i64({{.*}}!cheri.access_narrow
    // skip-NOT: llvm.cheri.cap.bounds.set
    // untrusted-NOT: llvm.cheri.cap.bounds.set
    // CHECK: ret i8
    s.as_bytes()[i]
}

// CHECK-LABEL: @boxed_slice_reborrow(
#[no_mangle]
pub fn boxed_slice_reborrow(b: &Box<[u64]>, i: usize) -> u64 {
    // noskip: @llvm.cheri.cap.bounds.set.i64({{.*}}!cheri.access_narrow
    // skip-NOT: llvm.cheri.cap.bounds.set
    // untrusted-NOT: !mem.alloc_inbounds
    // CHECK: ret i64
    let s: &[u64] = b;
    s[i]
}

// CHECK-LABEL: @vec_sum(
#[no_mangle]
pub fn vec_sum(v: &Vec<u64>) -> u64 {
    // M5r R3 (D23): the generic passes prove `i < len` and delete the check (and its marker)
    // before the late pass, so noskip has nothing left to narrow here (M5.0 narrowed early).
    // Recorded as an open decision in RESULTS-m5r.3.md.
    // noskip-NOT: llvm.cheri.cap.bounds.set
    // skip-NOT: llvm.cheri.cap.bounds.set
    // CHECK: ret i64
    let mut s = 0;
    for i in 0..v.len() {
        s += v[i];
    }
    s
}

// User raw parts: M3.3 conversion narrows to the stated length (contract_narrow).
// Never discharged: the operand is an untrusted raw pointer.
// CHECK-LABEL: @user_raw(
#[no_mangle]
pub unsafe fn user_raw(p: *const u64, n: usize, i: usize) -> u64 {
    // CHECK: @llvm.cheri.cap.bounds.set.i64({{.*}}!cheri.contract_narrow
    // CHECK: ret i64
    let s = unsafe { core::slice::from_raw_parts(p, n) };
    s[i]
}
