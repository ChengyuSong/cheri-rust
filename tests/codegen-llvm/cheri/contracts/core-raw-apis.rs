// M3.2 [LLM-generated]: core's raw-pointer APIs carry type contracts
// (`#[mem_contract(requires(...))]`); their out-of-line (-Copt-level=0) instances record the
// spatial precondition: "mem.range" on the pointer and the canonical "mem.contract".
// (Definitions get only the spatial facts by default; the `inline(always)` APIs such as
// ptr::read/write/copy/add have no out-of-line instance to carry them.)
//
//@ compile-flags: -Copt-level=0
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

// CHECK-DAG: define {{.*}}14from_raw_parts{{.*}}(ptr {{.*}}"mem.range"="1:8" %data, i64 {{.*}}%len, {{.*}}#[[FRP:[0-9]+]]
// CHECK-DAG: attributes #[[FRP]] = {{{.*}}"mem.contract"="slice(0,1,8,8)"
// CHECK-DAG: define {{.*}}18from_raw_parts_mut{{.*}}(ptr {{.*}}"mem.range"="1:8" %data, i64 {{.*}}%len, {{.*}}#[[FRPM:[0-9]+]]
// CHECK-DAG: attributes #[[FRPM]] = {{{.*}}"mem.contract"="slice_mut(0,1,8,8)"
// CHECK-DAG: define {{.*}}4CStr8from_ptr{{.*}}(ptr {{.*}}%ptr) {{.*}}#[[CSTR:[0-9]+]]
// CHECK-DAG: attributes #[[CSTR]] = {{{.*}}"mem.contract"="cstr(0)"
#[no_mangle]
pub unsafe fn t(p: *const u64, q: *mut u64, n: usize, s: *const core::ffi::c_char) -> u64 {
    unsafe {
        core::slice::from_raw_parts(p, n).len() as u64
            + core::slice::from_raw_parts_mut(q, n).len() as u64
            + core::ffi::CStr::from_ptr(s).count_bytes() as u64
    }
}
