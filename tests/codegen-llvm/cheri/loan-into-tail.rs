// M7.1b [LLM-generated]: may_dangle tail shapes. The Vec is not used after the
// referent is dropped, so the element loan ends at the last use of the Vec.
// `copy_from_slice` / `extend_from_slice` / `clone_from` must put every loan
// the source carries onto the destination, with that loan's value and id.
// At `-Copt-level=2`, SROA of a destination header rewrites its facts to
// `mem.loan_into.val` of the buffer-pointer SSA, with the same loan values
// and ids. A `CHECK-NOT` of `mem.loan_into` must include the quote and paren
// so it does not match that tag.
//
//@ revisions: o0 o2
//@ [o0] compile-flags: -Copt-level=0 -Cdebuginfo=0
//@ [o2] compile-flags: -Copt-level=2 -Cdebuginfo=0
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

#[derive(Copy, Clone)]
#[repr(C)]
struct Holder<'a> {
    r: &'a u64,
}

#[derive(Copy, Clone)]
#[repr(C)]
struct Pair<'a, 'b> {
    a: &'a u64,
    b: &'b u64,
}

// Two pushes. Each `&*` is two same-region reborrows, so two facts per value.
// o0-LABEL: @vec_tail(
// o0: mem.loan_into{{.*}}%v{{.*}}%[[D:[-a-zA-Z0-9_.]+]], i64
// o0: mem.loan_into{{.*}}%v{{.*}}%[[D]], i64
// o0: mem.loan_into{{.*}}%v{{.*}}%[[B:[-a-zA-Z0-9_.]+]], i64
// o0: mem.loan_into{{.*}}%v{{.*}}%[[B]], i64
// o0-NOT: "mem.loan_into"(
// o0: ret void
// o2-LABEL: @vec_tail(
// o2: mem.loan_into{{.*}}%v{{.*}}%[[D:[-a-zA-Z0-9_.]+]], i64
// o2: mem.loan_into{{.*}}%v{{.*}}%[[B:[-a-zA-Z0-9_.]+]], i64 [[BID:[0-9]+]]
// The next store is `black_box(*v[1])`, the last use of `v`. Its `!mem.last_use`
// node (checked after the last `-O2` function) lists this loan id.
// o2: store {{.*}}!mem.last_use ![[LU:[0-9]+]]
// o2-NOT: "mem.loan_into"(
// o2: ret void
#[no_mangle]
pub fn vec_tail() {
    let dummy = Box::new(0u64);
    let b = Box::new(1u64);
    let mut v: Vec<&u64> = Vec::with_capacity(2);
    v.push(&*dummy);
    v.push(&*b);
    core::hint::black_box(*v[1]);
    core::hint::black_box(b);
    core::hint::black_box(&dummy);
}

// Two reborrows of one Holder are two loads, so the SSA names differ.
// o0-LABEL: @holder_tail(
// o0: mem.loan_into{{.*}}%v,
// o0: mem.loan_into{{.*}}%v,
// o0: mem.loan_into{{.*}}%v,
// o0: mem.loan_into{{.*}}%v,
// o0-NOT: "mem.loan_into"(
// o0: ret void
// o2-LABEL: @holder_tail(
// o2: mem.loan_into{{.*}}%v{{.*}}%[[D:[-a-zA-Z0-9_.]+]], i64
// o2: mem.loan_into{{.*}}%v{{.*}}%[[B:[-a-zA-Z0-9_.]+]], i64
// o2-NOT: "mem.loan_into"(
// o2: ret void
#[no_mangle]
pub fn holder_tail() {
    let dummy = Box::new(0u64);
    let b = Box::new(1u64);
    let mut v: Vec<Holder<'_>> = Vec::with_capacity(2);
    v.push(Holder { r: &*dummy });
    v.push(Holder { r: &*b });
    core::hint::black_box(v[1].r);
    core::hint::black_box(b);
    core::hint::black_box(dummy);
}

// Field `a` (`ba`) and field `b` (`bb`) are different loan values. The first
// push stores the same dummy in both fields, so those facts share one value.
// o0-LABEL: @pair_tail(
// o0: mem.loan_into{{.*}}%v{{.*}}%[[D:[-a-zA-Z0-9_.]+]], i64
// o0: mem.loan_into{{.*}}%v{{.*}}%[[D]], i64
// o0: mem.loan_into{{.*}}%v{{.*}}%[[BA:[-a-zA-Z0-9_.]+]], i64
// o0: mem.loan_into{{.*}}%v{{.*}}%[[BA]], i64
// o0: mem.loan_into{{.*}}%v{{.*}}%[[BB:[-a-zA-Z0-9_.]+]], i64
// o0: mem.loan_into{{.*}}%v{{.*}}%[[BB]], i64
// o0-NOT: "mem.loan_into"(
// o0: ret void
// o2-LABEL: @pair_tail(
// o2: mem.loan_into{{.*}}%v{{.*}}%[[D:[-a-zA-Z0-9_.]+]], i64
// o2: mem.loan_into{{.*}}%v{{.*}}%[[BA:[-a-zA-Z0-9_.]+]], i64
// o2: mem.loan_into{{.*}}%v{{.*}}%[[BB:[-a-zA-Z0-9_.]+]], i64
// o2-NOT: "mem.loan_into"(
// o2: ret void
#[no_mangle]
pub fn pair_tail() {
    let dummy = Box::new(0u64);
    let ba = Box::new(1u64);
    let bb = Box::new(2u64);
    let mut v: Vec<Pair<'_, '_>> = Vec::with_capacity(2);
    v.push(Pair { a: &*dummy, b: &*dummy });
    v.push(Pair { a: &*ba, b: &*bb });
    core::hint::black_box(v[1].a);
    let rb = v[1].b;
    core::hint::black_box(ba);
    core::hint::black_box((&dummy, &bb, rb));
}

// Each src loan, including `&*b`, is repeated on dst with the same value and id.
// o0-LABEL: @copy_tail(
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S0:[-a-zA-Z0-9_.]+]], i64 [[I0:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S1:[-a-zA-Z0-9_.]+]], i64 [[I1:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S2:[-a-zA-Z0-9_.]+]], i64 [[I2:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S3:[-a-zA-Z0-9_.]+]], i64 [[I3:[0-9]+]]
// o0: call {{.*}}copy_from_slice
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S0]], i64 [[I0]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S1]], i64 [[I1]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S2]], i64 [[I2]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S3]], i64 [[I3]]
// o0-NOT: "mem.loan_into"(
// o0: ret void
// At `-O2` SROA promotes the destination header. Its facts are
// `mem.loan_into.val` of the buffer-pointer SSA, with the same loan values
// and ids as the source, including `&*b`.
// o2-LABEL: @copy_tail(
// o2: "mem.loan_into"({{.*}}%src{{.*}}%[[D:[-a-zA-Z0-9_.]+]], i64 [[ID:[0-9]+]]
// o2: "mem.loan_into"({{.*}}%src{{.*}}%[[B:[-a-zA-Z0-9_.]+]], i64 [[IB:[0-9]+]]
// o2: "mem.loan_into.val"({{.*}}%[[D]], i64 [[ID]]
// o2: "mem.loan_into.val"({{.*}}%[[B]], i64 [[IB]]
// o2-NOT: "mem.loan_into"(
// o2-NOT: "mem.loan_into.val"(
// o2: ret void
// o2: ![[LU]] = !{{{.*}}[[BID]]
#[no_mangle]
pub fn copy_tail() {
    let dummy = Box::new(0u64);
    let b = Box::new(1u64);
    let mut src: Vec<Holder<'_>> = Vec::with_capacity(2);
    src.push(Holder { r: &*dummy });
    src.push(Holder { r: &*b });
    let mut dst: Vec<Holder<'_>> = Vec::with_capacity(2);
    dst.push(Holder { r: &*dummy });
    dst.push(Holder { r: &*dummy });
    dst.copy_from_slice(&src);
    core::hint::black_box(src[1].r);
    core::hint::black_box(dst[1].r);
    core::hint::black_box(b);
    core::hint::black_box(dummy);
}

// o0-LABEL: @extend_tail(
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S0:[-a-zA-Z0-9_.]+]], i64 [[I0:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S1:[-a-zA-Z0-9_.]+]], i64 [[I1:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S2:[-a-zA-Z0-9_.]+]], i64 [[I2:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S3:[-a-zA-Z0-9_.]+]], i64 [[I3:[0-9]+]]
// o0: call {{.*}}extend_from_slice
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S0]], i64 [[I0]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S1]], i64 [[I1]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S2]], i64 [[I2]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S3]], i64 [[I3]]
// o0-NOT: "mem.loan_into"(
// o0: ret void
#[no_mangle]
pub fn extend_tail(_extra: &[Holder<'_>]) {
    let dummy = Box::new(0u64);
    let b = Box::new(1u64);
    let mut src: Vec<Holder<'_>> = Vec::with_capacity(2);
    src.push(Holder { r: &*dummy });
    src.push(Holder { r: &*b });
    let mut dst: Vec<Holder<'_>> = Vec::new();
    dst.extend_from_slice(&src);
    core::hint::black_box(dst[1].r);
    core::hint::black_box((b, dummy, _extra.len()));
}

// o0-LABEL: @clone_tail(
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S0:[-a-zA-Z0-9_.]+]], i64 [[I0:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S1:[-a-zA-Z0-9_.]+]], i64 [[I1:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S2:[-a-zA-Z0-9_.]+]], i64 [[I2:[0-9]+]]
// o0: mem.loan_into{{.*}}%src{{.*}}%[[S3:[-a-zA-Z0-9_.]+]], i64 [[I3:[0-9]+]]
// o0: call {{.*}}clone_from
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S0]], i64 [[I0]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S1]], i64 [[I1]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S2]], i64 [[I2]]
// o0: mem.loan_into{{.*}}%dst{{.*}}%[[S3]], i64 [[I3]]
// o0-NOT: "mem.loan_into"(
// o0: ret void
#[no_mangle]
pub fn clone_tail() {
    let dummy = Box::new(0u64);
    let b = Box::new(1u64);
    let mut src: Vec<Holder<'_>> = Vec::with_capacity(2);
    src.push(Holder { r: &*dummy });
    src.push(Holder { r: &*b });
    let mut dst: Vec<Holder<'_>> = Vec::new();
    dst.clone_from(&src);
    core::hint::black_box(dst[1].r);
    core::hint::black_box((b, dummy));
}
