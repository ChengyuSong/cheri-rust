// On CHERI purecap, liballoc bounds every global-allocator block to its allocation:
// the request is padded with CRRL, aligned with CRAM of the padded length, and the
// returned capability is narrowed with `llvm.cheri.cap.bounds.set.exact`. `dealloc`
// passes the same padded layout back. Software bounds checks are unchanged.
//
//@ only-riscv64cheri-unknown-none-purecap
//@ compile-flags: -Copt-level=2

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

// Constant sizes: CRRL/CRAM fold at the IR level.
// CHECK-LABEL: @boxed(
// CHECK: [[P:%.*]] = {{.*}}call {{.*}}@{{.*}}__rust_alloc{{.*}}(i64 noundef 32, i64 noundef 8)
// CHECK: call {{.*}}@llvm.cheri.cap.bounds.set.exact.i64(ptr addrspace(200) {{.*}}[[P]], i64 32)
#[no_mangle]
pub fn boxed() -> Box<[u64; 4]> {
    Box::new([7; 4])
}

// 0x3ff8 is not representable: padded to 0x4000 and aligned to 32.
// CHECK-LABEL: @boxed_carry(
// CHECK: [[P:%.*]] = {{.*}}call {{.*}}@{{.*}}__rust_alloc{{.*}}(i64 noundef 16384, i64 noundef 32)
// CHECK: call {{.*}}@llvm.cheri.cap.bounds.set.exact.i64(ptr addrspace(200) {{.*}}[[P]], i64 16384)
#[no_mangle]
pub fn boxed_carry() -> Box<[u8; 0x3ff8]> {
    Box::new([0; 0x3ff8])
}

// CHECK-LABEL: @with_capacity(
// CHECK: [[LEN:%.*]] = {{.*}}call {{.*}}i64 @llvm.cheri.round.representable.length.i64(
// CHECK: call {{.*}}@{{.*}}__rust_alloc{{.*}}(i64 noundef [[LEN]], i64
// CHECK: call {{.*}}@llvm.cheri.cap.bounds.set.exact.i64(ptr addrspace(200) {{.*}}, i64 [[LEN]])
#[no_mangle]
pub fn with_capacity(n: usize) -> Vec<u8> {
    Vec::with_capacity(n)
}

// CHECK-LABEL: @drop_box(
// CHECK: call {{.*}}@{{.*}}__rust_dealloc{{.*}}(ptr addrspace(200) {{.*}}%b, i64 noundef 32, i64 noundef 8)
#[no_mangle]
pub fn drop_box(b: Box<[u64; 4]>) {
    drop(b)
}

// The software bounds check stays (default `-Z cheri-cap-bounds-checks=off`).
// CHECK-LABEL: @index(
// CHECK: icmp ult i64 %i, %{{.*}}
// CHECK: panic_bounds_check
#[no_mangle]
pub fn index(v: &Vec<u64>, i: usize) -> u64 {
    v[i]
}
