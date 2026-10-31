// M3.2 [LLM-generated]: FFI-safe typed signatures are contracts with no annotation. rustc emits
// the same IR facts as the C header mirror (mem-contract-c): "mem.range" and a length range
// for slices, captures(address) for a parameter borrowed for the call (a late-bound lifetime
// used once and not in the output), a ret-borrow capture when it flows into the output,
// "mem.box_family" for Box, "mem.alloc_inbounds" on pointer returns, and (Q-M3.1b)
// "mem.alloc_inbounds" on typed extern "C" *declarations*; extern "C" *definitions* stay
// untrusted and are narrowed at entry (M3.0).
//
//@ compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=narrow
// M5r R3 (D23): CHECKs below see the late CHERI pass decisions [LLM-generated].
//@ compile-flags: -Cllvm-args=-cheri-bounds-lowering-in-opt
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![allow(improper_ctypes, improper_ctypes_definitions)]
extern crate alloc;
use alloc::boxed::Box;

pub struct Pair {
    pub a: u64,
    pub b: u64,
}

unsafe extern "Rust" {
    // CHECK-DAG: declare {{.*}}@r_sum(ptr {{.*}}noalias nofree noundef nonnull readonly align 8 captures(address) "mem.alloc_inbounds" "mem.range"="1:8"{{.*}}, i64 noundef range(i64 0, 1152921504606846976))
    fn r_sum(p: &[u64]) -> u64;
    // CHECK-DAG: declare {{.*}}@r_keep(ptr {{.*}}captures(address, read_provenance) dereferenceable(4) "mem.alloc_inbounds")
    fn r_keep(x: &'static u32);
    // CHECK-DAG: declare {{.*}}noundef nonnull align 4 "mem.alloc_inbounds" ptr {{.*}}@r_first(ptr {{.*}}captures(address, ret: address, provenance) "mem.alloc_inbounds" "mem.range"="1:4"
    fn r_first<'a>(p: &'a [u32]) -> &'a u32;
    // Two inputs share the region: either may flow wherever the other goes, so no
    // borrowed-for-the-call claim; rustc's default for a shared reference stays.
    // CHECK-DAG: declare {{.*}}@r_two(ptr {{.*}}captures(address, read_provenance) dereferenceable(4) "mem.alloc_inbounds", ptr {{.*}}captures(address, read_provenance) dereferenceable(4) "mem.alloc_inbounds")
    fn r_two<'a>(x: &'a u32, y: &'a u32);
    // CHECK-DAG: declare {{.*}}@r_box(ptr {{.*}}noalias noundef nonnull align 8 "mem.alloc_inbounds" "mem.box_family"="__rust_alloc")
    fn r_box(b: Box<Pair>);
    // CHECK-DAG: declare {{.*}}noundef nonnull align 8 "mem.alloc_inbounds" "mem.box_family"="__rust_alloc" ptr {{.*}}@r_new()
    fn r_new() -> Box<Pair>;
    // CHECK-DAG: declare {{.*}}@r_opt(ptr {{.*}}noalias nofree noundef readonly align 4 captures(address) dereferenceable_or_null(4) "mem.alloc_inbounds")
    fn r_opt(x: Option<&u32>) -> u32;
}

unsafe extern "C" {
    // Q-M3.1b: Rust calling C through a typed declaration trusts the C side's contract.
    // CHECK-DAG: declare {{.*}}@c_peek(ptr {{.*}}captures(address) dereferenceable(4) "mem.alloc_inbounds")
    fn c_peek(x: &u32) -> u32;
    // CHECK-DAG: declare {{.*}}@c_drop(ptr {{.*}}noalias noundef nonnull align 8 "mem.alloc_inbounds" "mem.box_family"="__rust_alloc")
    fn c_drop(b: Box<Pair>);
}

// A C-callable definition is not trusted: no "mem.alloc_inbounds" on the parameter.
// M5r R3 (D23): the entry narrow is placed after attribute inference, so the param may stay captures(none).
// CHECK-DAG: define {{.*}}@c_def(ptr {{.*}}captures({{(none|address)}}) dereferenceable(4) %{{[0-9a-z]+}})
#[no_mangle]
pub extern "C" fn c_def(x: &u32) -> u32 {
    *x
}

#[no_mangle]
pub fn use_all(p: &[u32], q: &[u64], x: &'static u32, b: Box<Pair>) -> u64 {
    unsafe {
        r_keep(x);
        r_two(x, x);
        r_box(b);
        c_drop(r_new());
        r_sum(q) + *r_first(p) as u64 + c_peek(x) as u64 + r_opt(Some(x)) as u64
    }
}
