// M7.2g [LLM-generated]: callee store marks. A user function whose body stores a
// pointer parameter (or the pointer it reads through a shared-reference parameter)
// at a constant offset from another pointer parameter gets
//   "mem.stores_into"="<key>:<off>" (or "<key>:<off>:*<d>") on the stored parameter
//   "mem.stores_into_dst"="<key>" on the destination parameter.
// A written `stores(p, into = place)` spec gives the same marks. A store at a
// variable offset gets none. The late CHERI pass turns a mark at a direct call into
// a caller-side `mem.store_into_slot` fact. `-Zcheri-stores-into-marks=no` emits no
// mark.
//
//@ revisions: on off
//@ compile-flags: -Cdebuginfo=0 -Copt-level=1
//@[off] compile-flags: -Zcheri-stores-into-marks=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![feature(rustc_attrs)]

extern crate alloc;

use alloc::boxed::Box;

#[repr(C)]
pub struct Hold {
    pub a: u64,
    pub p: *const u64,
}

// The pointer read through `b` (the Box's pointer) is stored 16 bytes into `r`.
// on-LABEL: define{{.*}}@put_box(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:16:*0"
// off-LABEL: define{{.*}}@put_box(
// off-NOT: mem.stores_into
// off: ret
#[no_mangle]
#[inline(never)]
pub fn put_box(r: *mut u8, b: &Box<u64>) {
    unsafe { (r.add(16) as *mut *const u64).write(&**b) }
}

// The parameter itself is stored through `dst`.
// on-LABEL: define{{.*}}@put_raw(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:0"
#[no_mangle]
#[inline(never)]
pub unsafe fn put_raw(dst: *mut *const u64, p: *const u64) {
    unsafe { *dst = p }
}

// A field of the pointee: offset of `Hold::p`.
// on-LABEL: define{{.*}}@put_field(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:16"
#[no_mangle]
#[inline(never)]
pub unsafe fn put_field(h: *mut Hold, p: *const u64) {
    unsafe { (*h).p = p }
}

// A variable offset: no mark.
// on-LABEL: define{{.*}}@put_var(
// on-NOT: mem.stores_into
// on: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn put_var(dst: *mut *const u64, i: usize, p: *const u64) {
    unsafe { *dst.add(i) = p }
}

// A written spec, body not visible to the analysis (it stores through a helper).
// on-LABEL: define{{.*}}@put_spec(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:16"
#[no_mangle]
#[inline(never)]
#[rustc_cheri_spec = "stores(p, into = h.p)"]
pub unsafe fn put_spec(h: *mut Hold, p: *const u64) {
    unsafe { helper(h, p) }
}

#[inline(never)]
unsafe fn helper(h: *mut Hold, p: *const u64) {
    unsafe { core::ptr::write_volatile(&raw mut (*h).p, p) }
}
