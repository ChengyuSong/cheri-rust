// M7.1d B2a [LLM-generated]: a store of a capability into a nameable local
// destination emits
//   call addrspace(200) void @llvm.mem.fact() [ "mem.store_into"(ptr addrspace(200) <dest>, ptr addrspace(200) <cap>) ]
// after the store. Destinations covered: a `ptr::read` dest, a `ptr::write`
// into a local, and a field of a live local struct. Statics, FFI, and
// `Box::leak` destinations are not emitted. The late CHERI pass may null
// `dest` at the source `mem.free`. Gated by `-Zcheri-annotate` and
// `-Zcheri-store-into` (default on; `no` emits nothing).
//
//@ revisions: on off
//@ compile-flags: -Copt-level=0 -Cdebuginfo=0
//@[off] compile-flags: -Zcheri-store-into=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::mem::MaybeUninit;
use core::ptr;

#[repr(C)]
struct RawHold {
    _pad: u64,
    p: *const u64,
}

// ptr::read into a named local. The dest is the local itself.
// on-LABEL: @read_local(
// on: "mem.store_into"
// off-LABEL: @read_local(
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn read_local(b: &u64) -> *const u64 {
    let mut v: Vec<&u64> = Vec::with_capacity(1);
    v.push(b);
    let copy: *const u64 = unsafe { ptr::read(v.as_ptr() as *const *const u64) };
    core::hint::black_box(&copy);
    copy
}

// ptr::write into a local MaybeUninit-style slot (a named local dest).
// on-LABEL: @write_local(
// on: "mem.store_into"
// off-LABEL: @write_local(
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn write_local(b: &u64) {
    let mut slot: *const u64 = ptr::null();
    unsafe { ptr::write(&mut slot, b as *const u64) };
    core::hint::black_box(&slot);
}

// Store into a field of a live local struct (init and a later ptr::write).
// on-LABEL: @field_write(
// on: "mem.store_into"
// off-LABEL: @field_write(
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn field_write(b: &u64) {
    let mut hold = RawHold { _pad: 0, p: ptr::null() };
    unsafe { ptr::write(&mut hold.p, b as *const u64) };
    core::hint::black_box(&hold);
}


// B2c: ptr::write into a capacity-only Vec<MaybeUninit<cap>> via as_mut_ptr().add(1).
// The dest is an indexed heap slot; rustc emits mem.store_into_slot, not a dest GEP.
// on-LABEL: @write_heap_slot(
// on: "mem.store_into_slot"
// off-LABEL: @write_heap_slot(
// off-NOT: "mem.store_into_slot"
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn write_heap_slot(b: &u64) {
    let cap: *const u64 = b;
    let mut raw: Vec<MaybeUninit<*const u64>> = Vec::with_capacity(2);
    unsafe { ptr::write(raw.as_mut_ptr().add(1), MaybeUninit::new(cap)) };
    core::hint::black_box(&raw);
}

// B2c: variable index into a live local array. Slot fact carries base + off.
// on-LABEL: @write_array_var(
// on: "mem.store_into_slot"
// off-LABEL: @write_array_var(
// off-NOT: "mem.store_into_slot"
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn write_array_var(b: &u64, i: usize) {
    let mut a: [*const u64; 2] = [ptr::null(), ptr::null()];
    unsafe { ptr::write(a.as_mut_ptr().add(i), b as *const u64) };
    core::hint::black_box((&a, i));
}
