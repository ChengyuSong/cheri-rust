// M3.2 [LLM-generated]: the export-trust entry narrow also covers Box<T>, Option<&T> and
// Option<Box<T>> parameters of C-callable functions: a capability wider than the referent is
// narrowed at entry, None stays None. `nofix` (-Zcheri-export-entry-bounds=no) is the M2
// behaviour: the wide capability reaches the body.
//@ run-pass
//@ revisions: fix nofix
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ aux-build: cheri_heap.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
//@ [nofix] compile-flags: -Zcheri-export-entry-bounds=no -Zcheri-contract-narrow=no
#![no_std]
#![no_main]
#![feature(stdarch_cheri)]
#![allow(improper_ctypes_definitions)]
// M3.6: transmute raw→typed is a conversion; allow lint; `#nofix` disables narrow.
#![allow(mem_contract_undischarged)]

extern crate alloc;
extern crate cheri_heap;
extern crate cheri_rt;

use alloc::boxed::Box;
use core::arch::cheri;
use core::hint::black_box;

static BIG: [u64; 64] = [7; 64]; // capability bounds: 512 bytes

#[inline(never)]
extern "C" fn c_opt(a: Option<&[u64; 4]>) -> (usize, u64) {
    match a {
        Some(a) => (cheri::length_get(a.as_ptr()), a[3]),
        None => (usize::MAX, 0),
    }
}
#[inline(never)]
extern "C" fn c_box(b: Box<[u64; 4]>) -> (usize, u64) {
    // `Box::into_raw` extracts Unique without the `&(*BoxDerefTransmute)` that
    // `Box::as_ptr` lowers to; that shape is an M3.3 thin `&*raw` conversion and
    // would setbounds to sizeof([u64;4]) (hiding the nofix case under noskip).
    let raw = Box::into_raw(b);
    let p = raw as *const u64;
    (cheri::length_get(p), unsafe { *p.add(3) })
}
#[inline(never)]
extern "C" fn c_opt_box(b: Option<Box<[u64; 4]>>) -> usize {
    match b {
        Some(b) => {
            let p = Box::into_raw(b) as *const u64;
            cheri::length_get(p)
        }
        None => usize::MAX,
    }
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let wide: *const u64 = black_box(BIG.as_ptr());
    assert_eq!(cheri::length_get(wide), 512);
    let r: &[u64; 4] = unsafe { core::mem::transmute(wide) };
    let f_opt: extern "C" fn(Option<&[u64; 4]>) -> (usize, u64) = black_box(c_opt);
    let (lo, vo) = f_opt(Some(r));
    assert_eq!(f_opt(None), (usize::MAX, 0)); // null is not narrowed into a tagged cap
    assert_eq!(vo, 7);

    // A Box<[u64; 4]> carrying the capability of a 128-byte allocation (never freed).
    // Built by transmute, not `Box::from_raw`: M3.3's conversion-narrow in
    // `from_raw` would establish the 32-byte referent and hide the nofix case.
    let big: Box<[u64; 16]> = Box::new([5; 16]);
    let p = Box::into_raw(big) as *mut [u64; 4];
    assert_eq!(cheri::length_get(p as *const u64), 128);
    let f_box: extern "C" fn(Box<[u64; 4]>) -> (usize, u64) = black_box(c_box);
    let wide_box: Box<[u64; 4]> = unsafe { core::mem::transmute(black_box(p)) };
    let (lb, vb) = f_box(wide_box);
    assert_eq!(vb, 5);
    let f_ob: extern "C" fn(Option<Box<[u64; 4]>>) -> usize = black_box(c_opt_box);
    let wide_box2: Box<[u64; 4]> = unsafe { core::mem::transmute(black_box(p)) };
    let lob = f_ob(Some(wide_box2));
    assert_eq!(f_ob(None), usize::MAX);
    #[cfg(fix)]
    assert_eq!((lo, lb, lob), (32, 32, 32));
    #[cfg(nofix)]
    assert_eq!((lo, lb, lob), (512, 128, 128));
}
