// Allocation bounds on CHERI purecap: a constant-size heap request folds CRRL/CRAM into
// constants and costs one `csetboundsexact`; statics and escaping stack arrays are padded
// and aligned so that their bounds are exactly representable. 0x3ff8 bytes round up to
// 0x4000, which needs 32-byte alignment (LLVM's old mask said 16).
//
//@ assembly-output: emit-asm
//@ compile-flags: -Copt-level=2
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;

// CHECK-LABEL: boxed:
// CHECK-NOT: crrl
// CHECK-NOT: cram
// CHECK: li a0, 32
// CHECK: __rust_alloc
// CHECK: csetboundsexact
#[no_mangle]
pub fn boxed() -> Box<[u64; 4]> {
    Box::new([7; 4])
}

unsafe extern "Rust" {
    fn sink(p: *const u8);
}

// CHECK-LABEL: stack_carry:
// CHECK: andi {{.*}}, -32
// CHECK: lui [[L:.*]], 4
// CHECK: csetbounds {{.*}}, [[L]]
#[no_mangle]
pub fn stack_carry() {
    let a = [1u8; 0x3ff8];
    unsafe { sink(a.as_ptr()) }
}

#[no_mangle]
pub static mut BEFORE: [u8; 40] = [0; 40];
// CHECK: .p2align 5
// CHECK-NEXT: CARRY:
// CHECK-NEXT: .zero 16376
// CHECK-NEXT: .zero 8
#[no_mangle]
pub static mut CARRY: [u8; 0x3ff8] = [0; 0x3ff8];
