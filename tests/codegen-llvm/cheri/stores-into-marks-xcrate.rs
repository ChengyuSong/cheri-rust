// M7.2h [LLM-generated]: callee store marks across crates. The marks of non-generic
// functions are kept in crate metadata, so the declaration of an upstream function
// in this crate gets the same "mem.stores_into" / "mem.stores_into_dst" parameter
// marks as in its own crate, and a local function that calls it gets a mark through
// the callee summary. `-Zcheri-stores-into-marks-xcrate=no` reads no upstream mark.
//
//@ revisions: on off
//@ aux-build:stores_into_marks_aux.rs
//@ compile-flags: -Cdebuginfo=0 -Copt-level=1
//@[off] compile-flags: -Zcheri-stores-into-marks-xcrate=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate stores_into_marks_aux as aux;

// A local wrapper: its mark comes from the upstream mark of `put_field_x`.
// on-LABEL: define{{.*}}@wrap_field(
// on-SAME: "mem.stores_into_dst"="0"
// on-SAME: "mem.stores_into"="0:16"
// off-LABEL: define{{.*}}@wrap_field(
// off-NOT: mem.stores_into
// off: ret
#[no_mangle]
#[inline(never)]
pub unsafe fn wrap_field(h: *mut aux::Hold, p: *const u64) {
    unsafe { aux::put_field_x(h, p) }
}

#[no_mangle]
pub unsafe fn call_all(d: *mut *const u64, h: *mut aux::Hold, p: *const u64) {
    unsafe {
        aux::put_raw_x(d, p);
        aux::put_var_x(d, 3, p);
    }
}

// The upstream declarations.
// on-DAG: declare{{.*}}@put_field_x(ptr addrspace(200) {{.*}}"mem.stores_into_dst"="0"{{.*}}, ptr addrspace(200) {{.*}}"mem.stores_into"="0:16"
// on-DAG: declare{{.*}}@put_raw_x(ptr addrspace(200) {{.*}}"mem.stores_into_dst"="0"{{.*}}, ptr addrspace(200) {{.*}}"mem.stores_into"="0:0"
// A variable offset: no mark upstream either.
// on-DAG: declare{{.*}}@put_var_x(ptr addrspace(200) noundef, i64 noundef, ptr addrspace(200) noundef)
// off-DAG: declare{{.*}}@put_field_x(ptr addrspace(200) noundef, ptr addrspace(200) noundef)
// off-DAG: declare{{.*}}@put_raw_x(ptr addrspace(200) noundef, ptr addrspace(200) noundef)
