// M7.1b [LLM-generated]: a call that can store a loan into a local Vec emits
// `call addrspace(200) void @llvm.mem.fact() [ "mem.loan_into"(ptr addrspace(200) <owner ptr field>, ptr addrspace(200) <loan value>, i64 <id>) ]`
// after the call. The id is the same hashed borrowck id as `!mem.last_use`.
// The late pass re-reads the header and nulls only slots equal to the loan value.
// Gated by `-Zcheri-annotate` on CHERI targets.
//
//@ revisions: on off
//@ compile-flags: -Copt-level=0 -Cdebuginfo=0
//@[off] compile-flags: -Zcheri-annotate=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::vec::Vec;

#[repr(C)]
struct Holder<'a> {
    r: &'a u64,
}

#[repr(C)]
struct Pair<'a, 'b> {
    a: &'a u64,
    b: &'b u64,
}

// `push(&*b)` : one loan, one fact on the Vec's buffer-pointer field.
// on-LABEL: @loan_push(
// on: call {{.*}}push{{.*}}
// on: mem.loan_into
// off-LABEL: @loan_push(
// off-NOT: mem.loan_into
// off: ret void
#[no_mangle]
pub fn loan_push(b: &u64) {
    let mut v: Vec<&u64> = Vec::with_capacity(1);
    v.push(&*b);
    core::hint::black_box(&v);
}

// Holder stores the reference in a field. The fact's loan value is that field.
// on-LABEL: @loan_holder(
// on: call {{.*}}push{{.*}}
// on: mem.loan_into
// off-LABEL: @loan_holder(
// off-NOT: mem.loan_into
// off: ret void
#[no_mangle]
pub fn loan_holder(b: &u64) {
    let mut v: Vec<Holder<'_>> = Vec::with_capacity(1);
    v.push(Holder { r: &*b });
    core::hint::black_box(&v);
}

// cap 1, second push grows inside push (grow_one stays a call in that callee).
// The fact names this Vec's pointer field; the late pass re-reads it after grow.
// on-LABEL: @loan_grow(
// on: mem.loan_into
// on: mem.loan_into
// on-NOT: mem.loan_into
// on: ret void
// off-LABEL: @loan_grow(
// off-NOT: mem.loan_into
// off: ret void
#[no_mangle]
pub fn loan_grow(b: &u64) {
    let r = &*b;
    let mut v: Vec<&u64> = Vec::with_capacity(1);
    v.push(r);
    v.push(r);
    core::hint::black_box(&v);
}

// Two regions, two reference values, two facts. 'b is not the 'a value.
// on-LABEL: @loan_pair(
// on: mem.loan_into
// on: mem.loan_into
// on-NOT: mem.loan_into
// on: ret void
// off-LABEL: @loan_pair(
// off-NOT: mem.loan_into
// off: ret void
#[no_mangle]
pub fn loan_pair(a: &u64, b: &u64) {
    let mut v: Vec<Pair<'_, '_>> = Vec::with_capacity(1);
    v.push(Pair { a: &*a, b: &*b });
    core::hint::black_box(&v);
}

// copy_from_slice propagates the source loan onto the destination Vec.
// Facts: push into src, push into dst, then one more after the copy.
// on-LABEL: @loan_copy(
// on: mem.loan_into
// on: mem.loan_into
// on: call {{.*}}copy_from_slice{{.*}}
// on: mem.loan_into
// on-NOT: mem.loan_into
// on: ret void
// off-LABEL: @loan_copy(
// off-NOT: mem.loan_into
// off: ret void
#[no_mangle]
pub fn loan_copy(b: &u64, c: &u64) {
    let mut src: Vec<Holder<'_>> = Vec::with_capacity(1);
    src.push(Holder { r: &*b });
    let mut dst: Vec<Holder<'_>> = Vec::with_capacity(1);
    dst.push(Holder { r: &*c });
    dst.copy_from_slice(&src);
    core::hint::black_box(&src);
    core::hint::black_box(&dst);
}

// `b` is a caller reference (no loan id here) stored beside a reborrow of the
// same place. The values can be equal, so the shorter loan gets no fact.
// on-LABEL: @guard_same_cap(
// on-NOT: mem.loan_into
// on: ret void
// off-LABEL: @guard_same_cap(
// off-NOT: mem.loan_into
// off: ret void
#[no_mangle]
pub fn guard_same_cap(b: &u64) {
    let mut v: Vec<&u64> = Vec::with_capacity(2);
    v.push(b);
    v.push(&*b);
    core::hint::black_box(&v);
}
