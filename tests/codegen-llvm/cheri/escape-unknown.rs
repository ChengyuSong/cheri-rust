// M7.2n [LLM-generated]: with `-Zcheri-noreuse`, a pointer that leaves to a place no
// fact names gets `mem.escape_unknown`(cap): a byte copy of a pointer-holding value
// whose length is not a constant (cap reloaded from the typed local), and a raw-pointer
// argument of an indirect call (fn pointer or `dyn` method). A reference argument, a
// constant-length copy, a direct call, and the caller's own frame address get none.
// Without the switch nothing is emitted.
//
//@ revisions: on off
//@ compile-flags: -Cdebuginfo=0 -Copt-level=1
//@[on] compile-flags: -Zcheri-noreuse
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

use core::hint::black_box;
use core::ptr;

pub trait Sink {
    fn put(&self, p: *const u64);
    fn put_ref(&self, p: &u64);
}

// on-LABEL: define{{.*}}@copy_dyn(
// on: call addrspace(200) void @llvm.memcpy
// on-NEXT: "mem.escape_unknown"(ptr addrspace(200) %b)
// on: ret
// off-LABEL: define{{.*}}@copy_dyn(
// off-NOT: mem.escape_unknown
// off: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn copy_dyn(r: *mut u8, b: &u64, n: usize) {
    let p: *const u64 = b;
    unsafe { ptr::copy_nonoverlapping((&raw const p) as *const u8, r, n) };
    black_box(&p);
}

// CHECK-LABEL: define{{.*}}@copy_const(
// CHECK-NOT: mem.escape_unknown
// CHECK: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn copy_const(r: *mut u8, b: &u64) {
    let p: *const u64 = b;
    unsafe { ptr::copy_nonoverlapping((&raw const p) as *const u8, r, 16) };
    black_box(&p);
}

// on-LABEL: define{{.*}}@call_fnptr(
// on: "mem.escape_unknown"(ptr addrspace(200) %p)
// on-NEXT: call {{.*}}%f(
// off-LABEL: define{{.*}}@call_fnptr(
// off-NOT: mem.escape_unknown
// off: ret
#[no_mangle]
#[inline(never)]
pub fn call_fnptr(f: fn(*const u64), p: *const u64) {
    f(p)
}

// on-LABEL: define{{.*}}@call_dyn(
// on: "mem.escape_unknown"(ptr addrspace(200) %p)
// on: ret
// off-LABEL: define{{.*}}@call_dyn(
// off-NOT: mem.escape_unknown
// off: ret
#[no_mangle]
#[inline(never)]
pub fn call_dyn(s: &dyn Sink, p: *const u64) {
    s.put(p)
}

// CHECK-LABEL: define{{.*}}@call_dyn_ref(
// CHECK-NOT: mem.escape_unknown
// CHECK: ret
#[no_mangle]
#[inline(never)]
pub fn call_dyn_ref(s: &dyn Sink, p: &u64) {
    s.put_ref(p)
}

// CHECK-LABEL: define{{.*}}@call_fnptr_frame(
// CHECK-NOT: mem.escape_unknown
// CHECK: ret
#[no_mangle]
#[inline(never)]
pub fn call_fnptr_frame(f: fn(*const u64)) {
    let v = black_box(5u64);
    f(&raw const v)
}

#[inline(never)]
fn direct(p: *const u64) {
    black_box(p);
}

// CHECK-LABEL: define{{.*}}@call_direct(
// CHECK-NOT: mem.escape_unknown
// CHECK: ret
#[no_mangle]
#[inline(never)]
pub fn call_direct(p: *const u64) {
    direct(p)
}
