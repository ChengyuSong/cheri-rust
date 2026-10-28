//@ compile-flags: -Copt-level=0
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![feature(core_intrinsics)]

// CHECK-LABEL: @narrow(
#[no_mangle]
pub unsafe fn narrow(p: *const u8, n: usize) -> *const u8 {
    // CHECK: @llvm.cheri.cap.bounds.set.i64(
    unsafe { core::intrinsics::cheri_bounds_narrow(p, n) }
}

// CHECK-LABEL: @narrow_exact(
#[no_mangle]
pub unsafe fn narrow_exact(p: *const u8, n: usize) -> *const u8 {
    // CHECK: @llvm.cheri.cap.bounds.set.exact.i64(
    unsafe { core::intrinsics::cheri_bounds_narrow_exact(p, n) }
}

// CHECK-LABEL: @invalidate(
#[no_mangle]
pub unsafe fn invalidate(p: *const u8) -> *const u8 {
    // CHECK: @llvm.cheri.cap.tag.clear(
    unsafe { core::intrinsics::cheri_invalidate(p) }
}

// Wide pointer: the data capability is narrowed and the length metadata is kept.
// CHECK-LABEL: @narrow_slice(
#[no_mangle]
pub unsafe fn narrow_slice(p: *const [u8], n: usize) -> *const [u8] {
    // CHECK: @llvm.cheri.cap.bounds.set.i64(
    unsafe { core::intrinsics::cheri_bounds_narrow(p, n) }
}
