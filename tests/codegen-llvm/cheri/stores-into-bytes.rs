// M7.2m [LLM-generated]: a byte copy (`copy_nonoverlapping::<u8>`) out of a value that
// holds pointers is a store of those pointers into the destination at the same
// relative offsets: rustc emits `mem.store_into_slot`(dst, off, cap) tagged
// `!mem.store_into.bytes`, with `cap` reloaded from the typed local. Pointers that an
// earlier byte copy put into a local byte buffer are followed through a move and
// `black_box` to the next byte copy out of that buffer. A copy of a value without
// pointers, or of unknown length, gets no fact. `-Zcheri-store-into-bytes=no` gives
// none of these.
//
//@ revisions: on off
//@ compile-flags: -Cdebuginfo=0 -Copt-level=1
//@[off] compile-flags: -Zcheri-store-into-bytes=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

use core::hint::black_box;
use core::ptr;

#[repr(C, align(16))]
pub struct Pair {
    pub n: u64,
    pub p: *const u64,
}

#[repr(align(16))]
pub struct Buf([u8; 16]);

// on-LABEL: define{{.*}}@copy_ptr(
// on: call addrspace(200) void @llvm.memcpy
// on: "mem.store_into_slot"(ptr addrspace(200) {{.*}}, i64 0, ptr addrspace(200) {{.*}}) ], !mem.store_into.bytes
// off-LABEL: define{{.*}}@copy_ptr(
// off-NOT: mem.store_into.bytes
// off: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn copy_ptr(r: *mut u8, b: &u64) {
    let p: *const u64 = b;
    let n = core::mem::size_of::<*const u64>();
    unsafe { ptr::copy_nonoverlapping((&raw const p) as *const u8, r.add(16), n) };
    black_box(&p);
}

// The pointer field is at offset 16 of the copied struct.
// on-LABEL: define{{.*}}@copy_struct(
// on: "mem.store_into_slot"(ptr addrspace(200) {{.*}}, i64 16, ptr addrspace(200) {{.*}}) ], !mem.store_into.bytes
// on-NOT: mem.store_into.bytes
// on: ret
// off-LABEL: define{{.*}}@copy_struct(
// off-NOT: mem.store_into.bytes
// off: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn copy_struct(r: *mut u8, b: &u64) {
    let pr = Pair { n: 7, p: b };
    unsafe { ptr::copy_nonoverlapping((&raw const pr) as *const u8, r, 32) };
    black_box(&pr);
}

// Through a local byte buffer and `black_box`: the copy out of the buffer gets a
// fact whose pointer is the one first copied in (`%b`).
// on-LABEL: define{{.*}}@via_buf(
// on: "mem.store_into_slot"(ptr addrspace(200) {{.*}}, i64 0, ptr addrspace(200) {{.*}}) ], !mem.store_into.bytes
// on: ret
// off-LABEL: define{{.*}}@via_buf(
// off-NOT: mem.store_into.bytes
// off: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn via_buf(r: *mut u8, b: &u64) {
    let p: *const u64 = b;
    let mut tmp = Buf([0u8; 16]);
    unsafe { ptr::copy_nonoverlapping((&raw const p) as *const u8, tmp.0.as_mut_ptr(), 16) };
    let tmp = black_box(tmp);
    unsafe { ptr::copy_nonoverlapping(tmp.0.as_ptr(), r.add(16), 16) };
}

// No pointers in the source, or a length that is not a constant: no fact.
// CHECK-LABEL: define{{.*}}@copy_int(
// CHECK-NOT: mem.store_into.bytes
// CHECK: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn copy_int(r: *mut u8, v: u64) {
    unsafe { ptr::copy_nonoverlapping((&raw const v) as *const u8, r, 8) };
    black_box(&v);
}

// CHECK-LABEL: define{{.*}}@copy_dyn(
// CHECK-NOT: mem.store_into.bytes
// CHECK: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn copy_dyn(r: *mut u8, b: &u64, n: usize) {
    let p: *const u64 = b;
    unsafe { ptr::copy_nonoverlapping((&raw const p) as *const u8, r, n) };
    black_box(&p);
}
