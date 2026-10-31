// M3.2 [LLM-generated]: the M3.0 entry narrow of C-callable definitions also covers Box<T>,
// Option<&T> and Option<Box<T>>. A C caller may pass a capability wider than the referent;
// the parameter is narrowed once at entry (null stays null for Option), and is not
// "mem.alloc_inbounds". Rust-ABI functions keep the trust.
//
//@ compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=narrow
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![allow(improper_ctypes_definitions)]
extern crate alloc;
use alloc::boxed::Box;

// CHECK-LABEL: define {{.*}}@c_box(
// CHECK-NOT: "mem.alloc_inbounds"
// CHECK-SAME: "mem.box_family"="__rust_alloc" [[B:%[0-9a-z._]+]])
// CHECK: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}[[B]], i64 32)
#[no_mangle]
pub extern "C" fn c_box(b: Box<[u64; 4]>) -> u64 {
    let r = b[1];
    core::mem::forget(b);
    r
}

// CHECK-LABEL: define {{.*}}@c_opt(
// CHECK-NOT: "mem.alloc_inbounds"
// CHECK-SAME: dereferenceable_or_null(32) [[X:%[0-9a-z._]+]])
// The null-preserving narrow (the speculatable bounds.set may be hoisted above the test).
// CHECK-DAG: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) [[X]], i64 32)
// CHECK-DAG: icmp eq ptr addrspace(200) [[X]], null
#[no_mangle]
pub extern "C" fn c_opt(x: Option<&[u64; 4]>) -> u64 {
    match x {
        Some(a) => a[1],
        None => 0,
    }
}

// CHECK-LABEL: define {{.*}}@c_opt_box(
// CHECK-NOT: "mem.alloc_inbounds"
// CHECK-SAME: [[X:%[0-9a-z._]+]])
// CHECK-DAG: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) [[X]], i64 32)
// CHECK-DAG: icmp eq ptr addrspace(200) [[X]], null
#[no_mangle]
pub extern "C" fn c_opt_box(x: Option<Box<[u64; 4]>>) -> u64 {
    match x {
        Some(a) => {
            let r = a[1];
            core::mem::forget(a);
            r
        }
        None => 0,
    }
}

// Rust ABI: trusted, no entry narrow.
// CHECK-LABEL: define {{.*}}@r_opt(
// CHECK-SAME: "mem.alloc_inbounds" %{{.*}})
// CHECK-NOT: llvm.cheri.cap.bounds.set
// CHECK: ret i64
#[no_mangle]
pub fn r_opt(x: Option<&[u64; 4]>) -> u64 {
    match x {
        Some(a) => a[1],
        None => 0,
    }
}
