// Exactness classes for `-Z cheri-cap-bounds-checks=elide` on RV64 (ISAv9, 128-bit caps):
// lengths up to 4095 bytes are exact for any base; above that the base and length must be
// aligned (L/512 to L/1024); a length that is not itself representable is never exact.
//
// The M2 skip pass is disabled so the M1 narrows of the checked fallback stay visible.
//@ compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=elide -Cllvm-args=-cheri-skip-narrow=false
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

// 4095 bytes: exact for any base, check removed.
// CHECK-LABEL: @len_4095(
#[no_mangle]
pub fn len_4095(a: &[u8; 4095], i: usize) -> u8 {
    // CHECK-NOT: icmp
    // CHECK: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 4095)
    // CHECK-NOT: panic_bounds_check
    // CHECK: ret i8
    a[i]
}

// 4097 bytes is never representable (it rounds to 4104): keep the check, narrow with
// rounding only.
// CHECK-LABEL: @len_4097(
#[no_mangle]
pub fn len_4097(a: &[u8; 4097], i: usize) -> u8 {
    // CHECK-NOT: llvm.cheri.cap.bounds.set.exact
    // CHECK: icmp
    // CHECK-NOT: llvm.cheri.cap.bounds.set.exact
    // CHECK: @llvm.cheri.cap.bounds.set.i64({{.*}}, i64 4097)
    // CHECK-NOT: llvm.cheri.cap.bounds.set.exact
    // CHECK: panic_bounds_check
    a[i]
}

// 4096 bytes needs an 8-byte aligned base; a `u8` array does not promise that, so the tag
// decides at run time.
// CHECK-LABEL: @len_4096_align1(
#[no_mangle]
pub fn len_4096_align1(a: &[u8; 4096], i: usize) -> u8 {
    // CHECK: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 4096)
    // CHECK: @llvm.cheri.cap.tag.get(
    // CHECK: cheri_inexact_bounds:
    // CHECK: panic_bounds_check
    a[i]
}

// The same length with an 8-byte aligned element is statically exact.
// CHECK-LABEL: @len_4096_align8(
#[no_mangle]
pub fn len_4096_align8(a: &[u64; 512], i: usize) -> u64 {
    // CHECK-NOT: icmp
    // CHECK: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 4096)
    // CHECK-NOT: llvm.cheri.cap.tag.get
    // CHECK-NOT: panic_bounds_check
    // CHECK: ret i64
    a[i]
}

// 8192 bytes needs 16-byte alignment (L/512), more than `u64` gives.
// CHECK-LABEL: @len_8192_align8(
#[no_mangle]
pub fn len_8192_align8(a: &[u64; 1024], i: usize) -> u64 {
    // CHECK: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 8192)
    // CHECK: @llvm.cheri.cap.tag.get(
    // CHECK: panic_bounds_check
    a[i]
}

#[repr(C, align(16))]
pub struct A16(pub [u64; 1024]);

// CHECK-LABEL: @len_8192_align16(
#[no_mangle]
pub fn len_8192_align16(a: &A16, i: usize) -> u64 {
    // CHECK-NOT: icmp
    // CHECK: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 8192)
    // CHECK-NOT: llvm.cheri.cap.tag.get
    // CHECK-NOT: panic_bounds_check
    // CHECK: ret i64
    a.0[i]
}

// 8184 bytes (1023 x u64) is exact with 8-byte alignment.
// CHECK-LABEL: @len_8184_align8(
#[no_mangle]
pub fn len_8184_align8(a: &[u64; 1023], i: usize) -> u64 {
    // CHECK-NOT: icmp
    // CHECK: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 8184)
    // CHECK-NOT: panic_bounds_check
    // CHECK: ret i64
    a[i]
}
