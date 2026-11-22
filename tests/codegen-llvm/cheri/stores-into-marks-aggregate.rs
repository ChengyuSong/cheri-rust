// M7.2k [LLM-generated]: callee store marks for stores of a whole value that holds a
// pointer. Each pointer field of the stored value that is a parameter (or a parameter
// of pointer-sized scalar type, like `Option<&T>`) gives a mark at the field's byte
// offset; fields that are not parameters give none. `MaybeUninit::write` of
// `Some(p)` counts too. `-Zcheri-stores-into-marks-aggregate=no` gives none of these.
//
//@ revisions: on off
//@ compile-flags: -Cdebuginfo=0 -Copt-level=1
//@[off] compile-flags: -Zcheri-stores-into-marks-aggregate=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

use core::mem::MaybeUninit;

#[repr(C)]
pub struct Pair {
    pub n: u64,
    pub a: *const u64,
    pub b: *const u64,
}

static G: u64 = 7;

// on-LABEL: define{{.*}}@put_opt(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:0"
// off-LABEL: define{{.*}}@put_opt(
// off-NOT: mem.stores_into
// off: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn put_opt<'a>(q: *mut Option<&'a u64>, p: &'a u64) {
    unsafe { *q = Some(p) }
}

// Only `a` (offset 16) is the parameter; `b` points to a static: one mark.
// on-LABEL: define{{.*}}@put_pair(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:16"{{[ )]}}
// off-LABEL: define{{.*}}@put_pair(
// off-NOT: mem.stores_into
// off: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn put_pair(q: *mut Pair, p: *const u64) {
    unsafe { *q = Pair { n: 1, a: p, b: &G } }
}

// A pointer-sized scalar parameter (`Option<&u64>`) stored as a tuple field.
// on-LABEL: define{{.*}}@put_tuple(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:16"
#[no_mangle]
#[inline(never)]
pub unsafe fn put_tuple<'a>(q: *mut (u64, Option<&'a u64>), p: Option<&'a u64>) {
    unsafe { *q = (7, p) }
}

// on-LABEL: define{{.*}}@put_mu(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:0"
// off-LABEL: define{{.*}}@put_mu(
// off-NOT: mem.stores_into
// off: ret
#[no_mangle]
#[inline(never)]
pub fn put_mu<'a>(q: &mut MaybeUninit<Option<&'a u64>>, p: &'a u64) {
    q.write(Some(p));
}
