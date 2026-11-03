// M5r R8 (D63) [LLM-generated]: `let r = &v[i]` on a Vec is an inlined `Index::index` whose
// reborrows MIR GVN turns into a copy of the returned reference, right after the loan start.
// The copy's store into the address-taken slot of `r` carries `!mem.loan` (the slot fact the
// late CheriInvalidate pass needs for the loan-end clear, D36); `off` is the R7 behaviour.
//
//@ revisions: on off
//@ compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=narrow
//@[off] compile-flags: -Zcheri-slot-copy-start=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
extern crate alloc;
use alloc::vec::Vec;

unsafe extern "Rust" {
    safe fn sink(p: *const *const u64);
}

// CHECK-LABEL: @slot_copy(
#[unsafe(no_mangle)]
pub fn slot_copy(v: &Vec<u64>) -> u64 {
    let r: &u64 = &v[1];
    // on: store ptr addrspace(200) {{.*}}, ptr addrspace(200) %r{{.*}}!mem.loan
    // off-NOT: !mem.loan
    let pr = (&raw const r) as *const *const u64;
    sink(pr);
    *r
}
