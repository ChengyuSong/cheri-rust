// Bounding every heap block must not stop LLVM from removing unused
// allocations: InstCombine looks through `llvm.cheri.cap.bounds.set(.exact)`
// and folds constant CRRL/CRAM.
//
//@ only-riscv64cheri-unknown-none-purecap
//@ compile-flags: -Copt-level=2

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;

// CHECK-LABEL: @boxed_sum(
// CHECK-NOT: __rust_alloc
// CHECK-NOT: llvm.cheri.cap.bounds.set
// CHECK-NOT: __rust_dealloc
// CHECK: ret i64
#[no_mangle]
pub fn boxed_sum(x: u64) -> u64 {
    let b = Box::new(x);
    *b + 1
}

// CHECK-LABEL: @vec_sum(
// Iteration goes through `ptr::read`'s M3.3 contract_narrow; CheriContractDischarge
// + a late InstCombine fold the loads so the sum is constant. Full alloc/dealloc
// elision is best-effort after that late pass (null-check CFG may remain).
// CHECK-NOT: !cheri.contract_narrow
// CHECK: ret i64 6
#[no_mangle]
pub fn vec_sum() -> u64 {
    let v = alloc::vec![1u64, 2, 3];
    v.iter().sum()
}
