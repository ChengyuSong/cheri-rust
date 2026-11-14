// M7.1d B3a [LLM-generated]: data-capability arguments of an `extern "C"`
// call are marked
//   call @stash(ptr addrspace(200) "mem.ffi_borrow" %p)
// or `"mem.ffi_retain"` for an `into_raw` hand-off / `#[cheri::ffi_retain]`.
// Function-pointer arguments are not marked. The late CHERI pass drops
// Global on borrow args; rustc only emits the fact.
// Gated by `-Zcheri-annotate` and `-Zcheri-ffi-narrow` (default on;
// `no` emits nothing).
//
//@ revisions: on off
//@ compile-flags: -Copt-level=0 -Cdebuginfo=0
//@[off] compile-flags: -Zcheri-ffi-narrow=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;

#[inline(never)]
#[unsafe(no_mangle)]
extern "C" fn stash(p: *const u64) {
    let _ = p;
}

#[inline(never)]
#[unsafe(no_mangle)]
#[cheri::ffi_retain]
extern "C" fn stash_retain(p: *const u64) {
    let _ = p;
}

#[inline(never)]
#[unsafe(no_mangle)]
#[cheri::ffi_borrow]
extern "C" fn stash_borrow(p: *const u64) {
    let _ = p;
}

type Cb = extern "C" fn(*const u64);

#[inline(never)]
#[unsafe(no_mangle)]
extern "C" fn register_cb(cb: Cb) {
    let _ = cb;
}

// Default borrow at a same-DSO no_mangle extern "C" boundary.
// on-LABEL: @borrow_arg(
// on: call {{.*}}@stash({{.*}}"mem.ffi_borrow"{{.*}})
// off-LABEL: @borrow_arg(
// off-NOT: mem.ffi_borrow
// off-NOT: mem.ffi_retain
// off: ret
#[unsafe(no_mangle)]
pub fn borrow_arg(b: &u64) {
    stash(b as *const u64);
}

// #[cheri::ffi_borrow] overrides into_raw (explicit borrow wins).
// on-LABEL: @borrow_overrides_into_raw(
// on: call {{.*}}@stash_borrow({{.*}}"mem.ffi_borrow"{{.*}})
// on-NOT: mem.ffi_retain
// off-LABEL: @borrow_overrides_into_raw(
// off-NOT: mem.ffi_borrow
// off: ret
#[unsafe(no_mangle)]
pub fn borrow_overrides_into_raw() {
    let b = Box::new(1u64);
    let p = Box::into_raw(b);
    stash_borrow(p as *const u64);
}

// Function-pointer arguments are not narrowed / marked.
// on-LABEL: @register_fnptr(
// on-NOT: mem.ffi_borrow
// on-NOT: mem.ffi_retain
// on: call {{.*}}@register_cb(
// on: ret
#[unsafe(no_mangle)]
pub fn register_fnptr() {
    register_cb(stash);
}

// Explicit #[cheri::ffi_retain] on the callee.
// on-LABEL: @retain_attr(
// on: call {{.*}}@stash_retain({{.*}}"mem.ffi_retain"{{.*}})
// on-NOT: mem.ffi_borrow
// off-LABEL: @retain_attr(
// off-NOT: mem.ffi_retain
// off: ret
#[unsafe(no_mangle)]
pub fn retain_attr(b: &u64) {
    stash_retain(b as *const u64);
}

// Automatic retain at Box::into_raw.
// on-LABEL: @retain_into_raw(
// on: call {{.*}}@stash({{.*}}"mem.ffi_retain"{{.*}})
// on-NOT: mem.ffi_borrow
// off-LABEL: @retain_into_raw(
// off-NOT: mem.ffi_retain
// off-NOT: mem.ffi_borrow
// off: ret
#[unsafe(no_mangle)]
pub fn retain_into_raw() {
    let b = Box::new(1u64);
    let p = Box::into_raw(b);
    stash(p as *const u64);
}
