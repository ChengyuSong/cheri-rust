// D114-rustc [LLM-generated]: a borrowck `LoanStart` sits immediately before the
// `Rvalue::Ref` of `&a[i]` / `&mut a[i]`. `find_access` must skip that marker (and
// `LoanEnd`) and still match the borrow, so the `Assert(BoundsCheck)` carries a
// `"mem.check"` with a non-null base, a nonzero element size, and the borrow kind
// flag (`CHKF_BORROW | CHK_INDEX` = 256), and the index GEP carries `!mem.guard`.
//
// `mir-opt-level=0` keeps the deref-separator copy that the negative case relies on
// (CopyProp would fold it back into the indexed place). Loan markers are inserted
// before those opts, so the positive cases still see them. `-Copt-level=0` leaves
// rustc's markers in the IR; this file does not enable CheriBoundsLowering.
//
// FileCheck notes (validated against CHERI rustc): panic blocks are emitted after
// the success-path `ret`, so `ret` / `"mem.check"` / `!mem.guard` are all CHECK-DAG
// within each CHECK-LABEL range. Regexes must be wrapped in `{{ }}`.
//
//@ compile-flags: -Copt-level=0 -Zmir-opt-level=0 -Coverflow-checks=off -Cdebug-assertions=off
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
extern crate alloc;

use alloc::vec::Vec;

// `a[i].len()` is a shared borrow of the element. The success block is
// `loan_start` then `_ = &(*a)[i]`. Panic-block order is not fixed, so the
// marker, the guard, and the return are unordered.
//
// CHECK-LABEL: @one(
// CHECK-DAG: "mem.check"(ptr addrspace(200) {{(nonnull )?%[^,)]+}}, i64 {{[^,]+}}, i64 {{[1-9][0-9]*}}, i64 {{[0-9]+}}, i64 256)
// CHECK-DAG: !mem.guard
// CHECK-DAG: ret i64
#[unsafe(no_mangle)]
pub fn one(a: &mut [Vec<u8>], i: usize) -> usize {
    a[i].len()
}

// Same shape as `core::slice::reverse`'s `revswap`: two borrows, two checks,
// each guarding its own place (`a[i]` and `b[n - 1 - i]`).
//
// CHECK-LABEL: @revswap(
// CHECK-DAG: "mem.check"(ptr addrspace(200) {{(nonnull )?%[^,)]+}}, i64 {{[^,]+}}, i64 {{[1-9][0-9]*}}, i64 {{[0-9]+}}, i64 256)
// CHECK-DAG: "mem.check"(ptr addrspace(200) {{(nonnull )?%[^,)]+}}, i64 {{[^,]+}}, i64 {{[1-9][0-9]*}}, i64 {{[0-9]+}}, i64 256)
// CHECK-DAG: !mem.guard
// CHECK-DAG: !mem.guard
// CHECK-DAG: ret void
#[unsafe(no_mangle)]
pub fn revswap(a: &mut [Vec<u8>], b: &mut [Vec<u8>], n: usize) {
    let mut i = 0;
    while i < n {
        core::mem::swap(&mut a[i], &mut b[n - 1 - i]);
        i += 1;
    }
}

// Negative: `&mut &mut [u64]` indexing is `(*(*a))[i]`. Derefer (required, even at
// mir-opt-level 0) copies the outer reference into a temp immediately before the
// indexed access. That copy is a real `Assign`, so the site stays unmatched:
// null base, element size 0, kind 0, and no `!mem.guard`.
//
// CHECK-LABEL: @blocked(
// CHECK-NOT: !mem.guard
// CHECK: "mem.check"(ptr addrspace(200) null, i64 {{[^,]+}}, i64 0, i64 {{[0-9]+}}, i64 0)
// CHECK-NOT: !mem.guard
#[unsafe(no_mangle)]
pub fn blocked(a: &mut &mut [u64], i: usize) -> u64 {
    a[i]
}
