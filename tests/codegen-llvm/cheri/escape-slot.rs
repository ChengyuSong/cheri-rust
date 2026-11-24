// M7.2o [LLM-generated]: with `-Zcheri-noreuse -Zcheri-noreuse-slots`, a byte copy of
// unknown length names the slot each carried pointer lands in:
// `mem.escape_unknown`(cap, dst + off). With `-Zcheri-noreuse` alone it is the M7.2n
// one-operand fact. A copy into a 16-aligned local at a misaligned offset (no tag can
// land) gets no fact with the slot switch. Indirect-call escapes keep one operand.
// Without the switches nothing is emitted.
//
//@ revisions: slots nr off
//@ compile-flags: -Cdebuginfo=0 -Copt-level=1
//@[slots] compile-flags: -Zcheri-noreuse -Zcheri-noreuse-slots
//@[nr] compile-flags: -Zcheri-noreuse
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

use core::hint::black_box;
use core::ptr;

#[repr(C)]
pub struct Pair {
    pub x: u64,
    pub p: *const u64,
}

// slots-LABEL: define{{.*}}@copy_dyn(
// slots: call addrspace(200) void @llvm.memcpy
// slots-NEXT: "mem.escape_unknown"(ptr addrspace(200) %b, ptr addrspace(200) %r)
// slots: ret
// nr-LABEL: define{{.*}}@copy_dyn(
// nr: "mem.escape_unknown"(ptr addrspace(200) %b) ]
// nr: ret
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

// The pointer sits at byte 16 of the source, so the slot is dst + 16.
// slots-LABEL: define{{.*}}@copy_dyn_pair(
// slots: call addrspace(200) void @llvm.memcpy
// slots: %[[S:[^ ]+]] = getelementptr i8, ptr addrspace(200) %r, i64 16
// slots: "mem.escape_unknown"(ptr addrspace(200) %{{[^,]+}}, ptr addrspace(200) %[[S]])
// slots: ret
// nr-LABEL: define{{.*}}@copy_dyn_pair(
// nr: "mem.escape_unknown"(ptr addrspace(200) %{{[^,]+}}) ]
// nr: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn copy_dyn_pair(r: *mut u8, b: &u64, n: usize) {
    let s = Pair { x: 1, p: b };
    unsafe { ptr::copy_nonoverlapping((&raw const s) as *const u8, r, n) };
    black_box(&s);
}

// CHECK-LABEL: define{{.*}}@call_fnptr(
// slots: "mem.escape_unknown"(ptr addrspace(200) %p) ]
// slots-NEXT: call {{.*}}%f(
// off-NOT: mem.escape_unknown
// CHECK: ret
#[no_mangle]
#[inline(never)]
pub fn call_fnptr(f: fn(*const u64), p: *const u64) {
    f(p)
}

#[repr(C)]
pub struct Buf {
    pub a: u64,
    pub b: [u8; 40],
    pub q: *const u64,
}

// Into a 16-aligned local at byte 8: no tag can land, so no fact with the slot switch.
// slots-LABEL: define{{.*}}@copy_local_mis(
// slots-NOT: mem.escape_unknown
// slots: ret
// nr-LABEL: define{{.*}}@copy_local_mis(
// nr: "mem.escape_unknown"(ptr addrspace(200) %b) ]
// nr: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn copy_local_mis(b: &u64, n: usize) {
    let p: *const u64 = b;
    let mut s = Buf { a: 0, b: [0; 40], q: ptr::null() };
    unsafe { ptr::copy_nonoverlapping((&raw const p) as *const u8, (&raw mut s.b) as *mut u8, n) };
    black_box((&p, &mut s));
}
