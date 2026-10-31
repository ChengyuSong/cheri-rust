// M3.3 [LLM-generated]: raw→typed conversion narrows. `from_raw_parts` emits a
// `llvm.cheri.cap.bounds.set` tagged `!cheri.contract_narrow` + `!mem.alloc_inbounds`.
// CheriSkipNarrow / CheriContractDischarge removes it when the operand is provably
// allocation-bounded. A C-supplied (untrusted) pointer keeps the narrow.
//
//@ revisions: skip noskip
//@ compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=narrow
//@ [noskip] compile-flags: -Cllvm-args=-cheri-skip-narrow=false
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![allow(improper_ctypes_definitions)]
#![allow(mem_contract_undischarged)]

// CHECK-LABEL: @raw_from_c(
// CHECK: call {{.*}}@llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) %{{.*}}, i64 %{{.*}}){{.*}}!cheri.contract_narrow
#[no_mangle]
pub unsafe extern "C" fn raw_from_c(p: *const u64, n: usize, i: usize) -> u64 {
    let s = unsafe { core::slice::from_raw_parts(p, n) };
    s[i]
}

// A typed Rust-ABI parameter is allocation-bounded; under skip the conversion
// narrow is discharged. Under noskip (-cheri-skip-narrow=false) discharge is
// off and the narrow remains (stronger mode).
// CHECK-LABEL: @from_trusted(
// skip-NOT: !cheri.contract_narrow
// noskip: !cheri.contract_narrow
#[no_mangle]
pub fn from_trusted(s: &[u64], i: usize) -> u64 {
    let s2 = unsafe { core::slice::from_raw_parts(s.as_ptr(), s.len()) };
    s2[i]
}

// Entry narrow of a C-callable typed export (not a contract_narrow).
// CHECK-LABEL: @typed_export(
// CHECK: call {{.*}}@llvm.cheri.cap.bounds.set.i64
// CHECK-NOT: !cheri.contract_narrow
#[no_mangle]
pub extern "C" fn typed_export(s: &[u64], i: usize) -> u64 {
    s[i]
}

// M3.6: transmute fat raw → `&[u64]` is a conversion-narrow site.
// CHECK-LABEL: @tm_from_c(
// CHECK: call {{.*}}@llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}%{{.*}}, i64 %{{.*}}){{.*}}!cheri.contract_narrow
#[no_mangle]
pub unsafe extern "C" fn tm_from_c(p: *const u64, n: usize, i: usize) -> u64 {
    let raw: *const [u64] = core::ptr::slice_from_raw_parts(p, n);
    let s: &[u64] = unsafe { core::mem::transmute(raw) };
    s[i]
}

// M4.0: typed-accessor shape for a C tagged union. The C accessor returns NULL on a
// tag mismatch, so Rust declares `Option<&T>`; the return is narrowed to the referent
// once and a null return stays null.
unsafe extern "C" {
    fn c_get_buf_acc(x: *const u8) -> Option<&'static [u64; 32]>;
}
// CHECK-LABEL: @acc_from_c(
// CHECK: [[R:%.*]] = {{.*}}call {{.*}}@c_get_buf_acc(
// CHECK: [[N:%.*]] = {{.*}}call {{.*}}@llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) [[R]], i64 256){{.*}}!cheri.contract_narrow
// CHECK: icmp eq ptr addrspace(200) [[R]], null
#[no_mangle]
pub unsafe extern "C" fn acc_from_c(x: *const u8, i: usize) -> u64 {
    match unsafe { c_get_buf_acc(x) } {
        Some(a) => a[i % 32],
        None => 0,
    }
}
