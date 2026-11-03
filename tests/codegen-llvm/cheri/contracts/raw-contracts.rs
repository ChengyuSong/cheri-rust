// M3.2 [LLM-generated]: raw-pointer contracts. `#[mem_contract(...)]` on extern declarations and
// `#[core::contracts::requires(valid_slice::<T>(p, n))]` on definitions lower the spatial
// predicates to the same IR facts as the C mirror, independently of -Zcontract-checks.
// Declarations get the full fact set; a definition by default only "mem.range" and the
// canonical "mem.contract" record (its callers are not checked until M3.3), and the full set
// with -Zcheri-contract-def-facts=yes.
//
//@ revisions: checks_no checks_yes deffacts
//@ compile-flags: -Copt-level=2 -Cno-prepopulate-passes
//@ [checks_no] compile-flags: -Zcontract-checks=no
//@ [checks_yes] compile-flags: -Zcontract-checks=yes
//@ [deffacts] compile-flags: -Zcheri-contract-def-facts=yes
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![feature(contracts)]
#![allow(incomplete_features)]
use core::contracts::{mem_contract, requires, valid_slice};
use core::ffi::c_char;

unsafe extern "C" {
    // CHECK-DAG: declare {{.*}}@c_crc(ptr {{.*}}noalias nofree noundef nonnull readonly captures(address) "mem.alloc_inbounds" "mem.range"="1:1", i64 noundef range(i64 0, -9223372036854775808)) {{.*}}#[[CRC:[0-9]+]]
    #[mem_contract(requires(valid_slice::<u8>(p, n)))]
    fn c_crc(p: *const u8, n: usize) -> u32;
    // CHECK-DAG: declare {{.*}}@c_reg(ptr {{.*}}captures(address, read_provenance) dereferenceable(4) "mem.alloc_inbounds") {{.*}}#[[REG:[0-9]+]]
    #[mem_contract(requires(valid_ref::<u32>(x)), keep(x))]
    fn c_reg(x: *const u32);
    // CHECK-DAG: declare {{.*}}noundef nonnull align 4 "mem.alloc_inbounds" ptr {{.*}}@c_first(ptr {{.*}}noalias nofree noundef nonnull align 4 captures(address, ret: address, provenance) "mem.alloc_inbounds" "mem.range"="1:4", i64 noundef range(i64 0, 2305843009213693952)) {{.*}}#[[FIRST:[0-9]+]]
    #[mem_contract(
        requires(valid_slice_mut::<u32>(p, n)),
        ret = valid_ref_mut::<u32>(ret),
        ret_borrows(p)
    )]
    fn c_first(p: *mut u32, n: usize) -> *mut u32;
    // CHECK-DAG: declare {{.*}}noundef nonnull align 4 "mem.alloc_inbounds" "mem.range"="0:4" ptr {{.*}}@c_table(i64 noundef range(i64 0, 2305843009213693952)) {{.*}}#[[TABLE:[0-9]+]]
    #[mem_contract(ret = valid_slice::<u32>(ret, n))]
    fn c_table(n: usize) -> *const u32;
    // CHECK-DAG: declare {{.*}}@c_strlen(ptr {{.*}}captures(address) "mem.alloc_inbounds" "mem.cstr") {{.*}}#[[STRLEN:[0-9]+]]
    #[mem_contract(requires(valid_cstr(s)))]
    fn c_strlen(s: *const c_char) -> usize;
}

// checks_no-DAG: define {{.*}}@sum_raw(ptr {{.*}}noundef "mem.range"="1:8" %p, i64 noundef %n) {{.*}}#[[DEF:[0-9]+]]
// checks_yes-DAG: define {{.*}}@sum_raw(ptr {{.*}}noundef "mem.range"="1:8" %{{.*}}, i64 noundef %{{.*}}) {{.*}}#[[DEF:[0-9]+]]
// checks_yes-DAG: call {{.*}}contract_check_requires
// deffacts-DAG: define {{.*}}@sum_raw(ptr {{.*}}noalias nofree noundef nonnull readonly align 8 captures(address) "mem.alloc_inbounds" "mem.range"="1:8" %p, i64 noundef range(i64 0, 1152921504606846976) %n) {{.*}}#[[DEF:[0-9]+]]
#[requires(valid_slice::<u64>(p, n))]
#[no_mangle]
pub unsafe fn sum_raw(p: *const u64, n: usize) -> u64 {
    let s = unsafe { core::slice::from_raw_parts(p, n) };
    s.iter().fold(0, |a, x| a.wrapping_add(*x))
}

#[no_mangle]
pub unsafe fn use_all(b: &[u8], q: *mut u32) -> u64 {
    unsafe {
        c_reg(c_table(1));
        c_crc(b.as_ptr(), b.len()) as u64
            + *c_first(q, 1) as u64
            + c_strlen(b.as_ptr().cast()) as u64
            + sum_raw(c_table(2).cast(), 1)
    }
}

// The canonical record, LLVM argument indices (C mirror format).
// CHECK-DAG: attributes #[[CRC]] = {{{.*}}"mem.contract"="slice(0,1,1,1)"
// CHECK-DAG: attributes #[[REG]] = {{{.*}}"mem.contract"="ref(0,4,4);keep(0)"
// CHECK-DAG: attributes #[[FIRST]] = {{{.*}}"mem.contract"="slice_mut(0,1,4,4);ref_mut(ret,4,4);ret_borrows(0)"
// CHECK-DAG: attributes #[[TABLE]] = {{{.*}}"mem.contract"="slice(ret,0,4,4)"
// CHECK-DAG: attributes #[[STRLEN]] = {{{.*}}"mem.contract"="cstr(0)"
// CHECK-DAG: attributes #[[DEF]] = {{{.*}}"mem.contract"="slice(0,1,8,8)"
