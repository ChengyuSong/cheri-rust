// M7.2i [LLM-generated]: callee store marks from `core` / `alloc` functions. A generic
// `core` function instantiated in this crate (here `core::mem::replace` and
// `Cell::set` at O0, where they stay calls) gets the same
// "mem.stores_into" / "mem.stores_into_dst" parameter marks as a function of this
// crate. `-Zcheri-stores-into-marks-lib=no` puts no mark on them.
//
//@ revisions: on off
//@ compile-flags: -Cdebuginfo=0 -Copt-level=0
//@[off] compile-flags: -Zcheri-stores-into-marks-lib=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

use core::cell::Cell;

#[no_mangle]
pub fn rep(d: &mut *const u64, p: *const u64) -> *const u64 {
    core::mem::replace(d, p)
}

#[no_mangle]
pub fn cell_set(c: &Cell<*const u64>, p: *const u64) {
    c.set(p);
}

// on-DAG: define{{.*}}core3mem7replace{{.*}}(ptr addrspace(200) {{.*}}"mem.stores_into_dst"="0"{{.*}}, ptr addrspace(200) {{.*}}"mem.stores_into"="0:0"
// on-DAG: define{{.*}}4Cell{{.*}}3set{{.*}}(ptr addrspace(200) {{.*}}"mem.stores_into_dst"="0"{{.*}}, ptr addrspace(200) {{.*}}"mem.stores_into"="0:0"
// off: define{{.*}}core3mem7replace
// off-NOT: mem.stores_into
// off: ret
