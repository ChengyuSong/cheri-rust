// M7.1d CR-1 [LLM-generated]: rustc emits a typic C-root table of
// C-visible writable data-capability statics (stored from an
// `extern "C"` body and/or exported as C data). In this crate that is
// `FFI_SLOT` only: `FFI_CB` is fn-ptr-only, a Rust-only typic static
// is a B2b dest not a C-root, and `-Zcheri-croot=no` emits nothing.
// The late CHERI pass walks the table at `mem.free`. The CR-3 symbol
// `cheri_register_root` is reserved as a declaration when the table
// is non-empty.
//
//@ revisions: on off
//@ compile-flags: -Cdebuginfo=0 -Copt-level=0
//@[off] compile-flags: -Zcheri-croot=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

use core::ptr;

static mut FFI_SLOT: *const u64 = core::ptr::null();
static mut FFI_CB: Option<extern "C" fn()> = None;
static mut RUST_ONLY: *const u64 = core::ptr::null();

#[unsafe(no_mangle)]
static mut EXPORT_SLOT: *const u64 = core::ptr::null();

#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn ffi_stash(p: *const u64) {
    unsafe { ptr::write_volatile(&raw mut FFI_SLOT, p) }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn ffi_set_cb(cb: extern "C" fn()) {
    unsafe { ptr::write_volatile(&raw mut FFI_CB, Some(cb)) }
}

// A Rust (non-extern "C") write_volatile into a typic static is a B2b
// dest, not a C-root. It must not appear in the table.
#[inline(never)]
#[unsafe(no_mangle)]
pub fn rust_stash(p: *const u64) {
    unsafe { ptr::write_volatile(&raw mut RUST_ONLY, p) }
}

// on: @__cheri_croot_slots = {{.*}}@EXPORT_SLOT{{.*}}FFI_SLOT
// on-NOT: FFI_CB
// on-NOT: RUST_ONLY
// on: @__cheri_croot =
// on: @cheri_register_root
// off-NOT: @__cheri_croot
// off-NOT: @__cheri_croot_slots
// off-NOT: @cheri_register_root
// off: ret
pub fn keep() {
    unsafe {
        core::hint::black_box(&raw const FFI_SLOT);
        core::hint::black_box(&raw const FFI_CB);
        core::hint::black_box(&raw const RUST_ONLY);
        core::hint::black_box(&raw const EXPORT_SLOT);
    }
}
