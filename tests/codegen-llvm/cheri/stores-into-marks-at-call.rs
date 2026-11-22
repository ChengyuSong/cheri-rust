// M7.2j [LLM-generated]: caller-side store_into facts for callee store marks, emitted
// by rustc right after the call. The callee here is `#[inline(always)]`, so LLVM
// inlines it and the late pass never sees the call; the fact rustc put after the
// call is still there, tagged `!mem.stores_into.mark` and `!mem.stores_into.at_call`.
// (`-Zinline-mir=no` keeps the calls in MIR, so the test sees the LLVM inliner only.)
// A ":*d" mark reloads the stored pointer through the shared-reference argument
// (shown at a call LLVM keeps; for an inlined user callee the same fact also comes
// from the callee body and the two are merged). A `core` callee (`Cell::set`, lib
// marks) gets the fact too: its body facts are not emitted in `core`.
// A destination in the caller's own frame (`&mut local`) gets no fact.
// `-Zcheri-stores-into-marks-at-call=no` emits none.
//
//@ revisions: on off
//@ compile-flags: -Cdebuginfo=0 -Copt-level=1 -Zinline-mir=no
//@[off] compile-flags: -Zcheri-stores-into-marks-at-call=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;

#[repr(C)]
pub struct Hold {
    pub a: u64,
    pub p: *const u64,
}

#[inline(always)]
unsafe fn put_field(h: *mut Hold, p: *const u64) {
    unsafe { (*h).p = p }
}

#[inline(never)]
fn put_box(r: *mut u8, b: &Box<u64>) {
    unsafe { *(r.add(16) as *mut *const u64) = &**b }
}

// on-LABEL: define{{.*}}@heap_dst(
// on: call addrspace(200) void @llvm.mem.fact() [ "mem.store_into_slot"(ptr addrspace(200) %h, i64 16, ptr addrspace(200) %p) ]{{.*}}!mem.stores_into.mark ![[E:[0-9]+]], !mem.stores_into.at_call ![[E]]
// on: ret
// off-LABEL: define{{.*}}@heap_dst(
// off-NOT: mem.stores_into.at_call
// off: ret
#[no_mangle]
pub unsafe fn heap_dst(h: *mut Hold, p: *const u64) {
    unsafe { put_field(h, p) }
}

// on-LABEL: define{{.*}}@deref_src(
// on: call{{.*}}@{{.*}}put_box
// on-NEXT: [[CAP:%.*]] = load ptr addrspace(200), ptr addrspace(200) %b, align 16
// on-NEXT: call addrspace(200) void @llvm.mem.fact() [ "mem.store_into_slot"(ptr addrspace(200) %r, i64 16, ptr addrspace(200) [[CAP]]) ]{{.*}}!mem.stores_into.at_call
// on: ret
#[no_mangle]
pub fn deref_src(r: *mut u8, b: &Box<u64>) {
    put_box(r, b)
}

// on-LABEL: define{{.*}}@cell_set(
// on: call addrspace(200) void @llvm.mem.fact() [ "mem.store_into_slot"(ptr addrspace(200) %c, i64 0, ptr addrspace(200) %p) ]{{.*}}!mem.stores_into.at_call
// on: ret
// off-LABEL: define{{.*}}@cell_set(
// off-NOT: mem.stores_into.at_call
// off: ret
#[no_mangle]
pub fn cell_set(c: &core::cell::Cell<*const u64>, p: *const u64) {
    c.set(p)
}

// on-LABEL: define{{.*}}@frame_dst(
// on-NOT: mem.stores_into.at_call
// on: ret
#[no_mangle]
pub fn frame_dst(p: *const u64) -> *const u64 {
    let mut h = Hold { a: 0, p: core::ptr::null() };
    unsafe { put_field(&mut h, p) };
    h.p
}
