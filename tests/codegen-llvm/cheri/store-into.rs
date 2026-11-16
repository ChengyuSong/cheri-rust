// M7.1d B2a / B2b-2 [LLM-generated]: a store of a capability into a
// nameable destination emits
//   call addrspace(200) void @llvm.mem.fact() [ "mem.store_into"(ptr addrspace(200) <dest>, ptr addrspace(200) <cap>) ]
// after the store. Destinations covered: a `ptr::read` dest, a `ptr::write`
// into a local, a field of a live local struct, and (B2b-2) a
// `ptr::write_volatile` into a rustc-named static DefId (or a field of
// one). Locals written with write_volatile (official trackb u0),
// `extern "C"` bodies (FFI_SLOT), and `Box::leak` destinations are not
// emitted. The late CHERI pass may null `dest` at the source `mem.free`.
// Gated by `-Zcheri-annotate` and `-Zcheri-store-into` (default on; `no`
// emits nothing).
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

// B2c-3a: Vec::push of a cap. Fact is emitted after the call, naming the
// post-grow heap slot (store_into_slot).
// on-LABEL: @push_cap(
// on: "mem.store_into_slot"
// off-LABEL: @push_cap(
// off-NOT: "mem.store_into_slot"
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn push_cap(b: &u64) {
    let mut v: Vec<&u64> = Vec::with_capacity(1);
    v.push(b);
    core::hint::black_box(&v);
}

// B2c-3a: IndexMut assignment into a reserved Vec.
// on-LABEL: @index_mut_assign(
// on: "mem.store_into_slot"
// off-LABEL: @index_mut_assign(
// off-NOT: "mem.store_into_slot"
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn index_mut_assign(b: &u64) {
    let mut v: Vec<*const u64> = Vec::with_capacity(1);
    v.push(ptr::null());
    v[0] = b as *const u64;
    core::hint::black_box(&v);
}

// B2c-3a: extend_from_slice of a local array of caps.
// on-LABEL: @extend_caps(
// on: "mem.store_into_slot"
// off-LABEL: @extend_caps(
// off-NOT: "mem.store_into_slot"
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn extend_caps(b: &u64, d: &u64) {
    let caps: [&u64; 2] = [d, b];
    let mut v: Vec<&u64> = Vec::with_capacity(2);
    v.extend_from_slice(&caps);
    core::hint::black_box(&v);
}

static mut STATIC_SLOT: *const u64 = core::ptr::null();
static mut STATIC_HOLD: RawHold = RawHold { _pad: 0, p: core::ptr::null() };

// B2b-2: write_volatile into a named static emits mem.store_into.
// on-LABEL: @write_volatile_static(
// on: "mem.store_into"
// off-LABEL: @write_volatile_static(
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub unsafe fn write_volatile_static(b: &u64) {
    ptr::write_volatile(&raw mut STATIC_SLOT, b as *const u64);
    core::hint::black_box(&raw const STATIC_SLOT);
}

// B2b-2: write_volatile into a field of a named static also emits.
// on-LABEL: @write_volatile_static_field(
// on: "mem.store_into"
// off-LABEL: @write_volatile_static_field(
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub unsafe fn write_volatile_static_field(b: &u64) {
    ptr::write_volatile(&raw mut STATIC_HOLD.p, b as *const u64);
    core::hint::black_box(&raw const STATIC_HOLD);
}

// B2b-2: write_volatile into a local must not emit (trackb u0).
// on-LABEL: @write_volatile_local(
// on-NOT: "mem.store_into"
// on: ret
// off-LABEL: @write_volatile_local(
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub fn write_volatile_local(b: &u64) {
    let mut slot: *const u64 = ptr::null();
    unsafe { ptr::write_volatile(&mut slot, b as *const u64) };
    core::hint::black_box(&slot);
}

// B2b-2: an extern "C" body writing a static (FFI_SLOT-like) must not emit.
// on-LABEL: @ffi_write_static(
// on-NOT: "mem.store_into"
// on: ret
// off-LABEL: @ffi_write_static(
// off-NOT: "mem.store_into"
// off: ret
#[no_mangle]
pub unsafe extern "C" fn ffi_write_static(b: *const u64) {
    ptr::write_volatile(&raw mut STATIC_SLOT, b);
}
