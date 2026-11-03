// Sites where `-Z cheri-cap-bounds-checks=elide` must keep the software bounds check.
//
//@ compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=elide
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

// Zero-sized elements: no memory access, so no hardware fault.
// CHECK-LABEL: @zst_elem(
#[no_mangle]
pub fn zst_elem(a: &[(); 4], i: usize) {
    // CHECK-NOT: llvm.cheri.cap.bounds.set
    // CHECK: icmp
    // CHECK: panic_bounds_check
    a[i]
}

// CHECK-LABEL: @zst_slice(
#[no_mangle]
pub fn zst_slice(s: &[()], i: usize) {
    // CHECK-NOT: llvm.cheri.cap.bounds.set
    // CHECK: icmp
    // CHECK: panic_bounds_check
    s[i]
}

// A zero-sized field of a non-ZST element is still not an access.
// CHECK-LABEL: @zst_field(
#[no_mangle]
pub fn zst_field(s: &[(u8, ())], i: usize) {
    // CHECK-NOT: llvm.cheri.cap.bounds.set
    // CHECK: icmp
    // CHECK: panic_bounds_check
    s[i].1
}

// Borrow only: Rust must panic at the borrow even if it is never dereferenced.
// CHECK-LABEL: @borrow_only(
#[no_mangle]
pub fn borrow_only(s: &[u8], i: usize) -> &u8 {
    // CHECK-NOT: llvm.cheri.cap.bounds.set
    // CHECK: icmp
    // CHECK: getelementptr inbounds nuw
    // CHECK: panic_bounds_check
    &s[i]
}

// CHECK-LABEL: @raw_borrow_only(
#[no_mangle]
pub fn raw_borrow_only(s: &[u8], i: usize) -> *const u8 {
    // CHECK-NOT: llvm.cheri.cap.bounds.set
    // CHECK: icmp
    // CHECK: panic_bounds_check
    &raw const s[i]
}

// Raw slice pointers are never narrowed: `*const [T]` may describe more elements than its
// capability covers.
// CHECK-LABEL: @raw_slice(
#[no_mangle]
pub unsafe fn raw_slice(p: *const [u8], i: usize) -> u8 {
    // CHECK-NOT: llvm.cheri.cap.bounds.set
    // CHECK: icmp
    // CHECK: panic_bounds_check
    unsafe { (*p)[i] }
}

// The loaded value is dead in MIR: LLVM would delete the load and the fault with it.
// CHECK-LABEL: @dead_load(
#[no_mangle]
pub fn dead_load(s: &[u8], i: usize) {
    // CHECK-NOT: llvm.cheri.cap.bounds.set
    // CHECK: icmp
    // CHECK: panic_bounds_check
    let _x = s[i];
}
