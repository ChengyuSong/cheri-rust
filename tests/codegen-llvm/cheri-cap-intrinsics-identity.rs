// Non-CHERI lowering is the pointer unchanged, with no `llvm.cheri` call.
//@ compile-flags: -Copt-level=0
//@ ignore-riscv64cheri-unknown-none-purecap
//@ ignore-riscv32cheriot-unknown-cheriotrtos

#![crate_type = "lib"]
#![no_std]
#![feature(core_intrinsics)]

// CHECK-LABEL: @cheri_bounds_narrow_id(
#[no_mangle]
pub unsafe fn cheri_bounds_narrow_id(p: *const u8, n: usize) -> *const u8 {
    // CHECK-NOT: llvm.cheri
    // CHECK: ret ptr
    unsafe { core::intrinsics::cheri_bounds_narrow(p, n) }
}

// CHECK-LABEL: @cheri_bounds_narrow_exact_id(
#[no_mangle]
pub unsafe fn cheri_bounds_narrow_exact_id(p: *const u8, n: usize) -> *const u8 {
    // CHECK-NOT: llvm.cheri
    // CHECK: ret ptr
    unsafe { core::intrinsics::cheri_bounds_narrow_exact(p, n) }
}

// CHECK-LABEL: @cheri_invalidate_id(
#[no_mangle]
pub unsafe fn cheri_invalidate_id(p: *const u8) -> *const u8 {
    // CHECK-NOT: llvm.cheri
    // CHECK: ret ptr
    unsafe { core::intrinsics::cheri_invalidate(p) }
}
