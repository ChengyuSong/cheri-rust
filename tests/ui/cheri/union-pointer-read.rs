//@ check-fail
//@ only-riscv64cheri-unknown-none-purecap
// M4.0 [LLM-generated]: union policy. In CHERI-contracted code (CHERI target with
// -Zcheri-contract-narrow, the default), reading a union field whose type is or
// contains a reference, slice, Box, raw pointer, NonNull or fn pointer (or a generic
// that may be one) is a hard error. A `requires` does not silence it. Plain-data
// unions, writes, wildcard patterns, `&raw` borrows and MaybeUninit stay allowed.
// (Was the Bu gadget: union pun of a raw fat pointer into &[u64].)
#![no_std]
#![crate_type = "lib"]
#![feature(contracts, contracts_internals, stmt_expr_attributes, proc_macro_hygiene)]
#![allow(internal_features, incomplete_features, unused_unsafe)]
use core::mem::MaybeUninit;
use core::ptr::NonNull;

pub union PunSlice<'a> { pub raw: *const [u64], pub typed: &'a [u64] }
pub union Plain { pub a: u64, pub b: [u8; 8], pub c: f64 }
pub struct Wrap<'a> { pub u: PunSlice<'a> }
pub union Agg { pub pair: (usize, &'static u8), pub n: [usize; 2] }
pub union FnU { pub f: fn() -> u32, pub n: usize }
pub union NnU { pub nn: NonNull<u8>, pub n: usize }
pub union G<T: Copy> { pub t: T, pub n: usize }

// --- rejected ---
pub unsafe fn bu<'a>(p: *const u64, n: usize, i: usize) -> u64 {
    let u = PunSlice { raw: core::ptr::slice_from_raw_parts(p, n) };
    let s: &[u64] = unsafe { u.typed }; //~ ERROR reading union field of type `&[u64]`
    s[i]
}
#[core::contracts::requires(core::contracts::valid_slice::<u64>(p, n))]
pub unsafe fn bu_with_requires<'a>(p: *const u64, n: usize) -> &'a [u64] {
    let u = PunSlice { raw: core::ptr::slice_from_raw_parts(p, n) };
    unsafe { u.typed } //~ ERROR reading union field of type `&[u64]`
}
pub unsafe fn raw_field(u: &PunSlice<'_>) -> *const [u64] {
    unsafe { u.raw } //~ ERROR reading union field of type `*const [u64]`
}
pub unsafe fn nested(w: &Wrap<'_>) -> usize {
    unsafe { w.u.typed.len() } //~ ERROR reading union field of type
}
pub unsafe fn pattern(u: PunSlice<'_>) -> usize {
    let PunSlice { typed } = u; //~ ERROR reading union field of type
    typed.len()
}
pub unsafe fn match_pattern(u: &PunSlice<'_>) -> usize {
    match *u {
        PunSlice { typed: s } => s.len(), //~ ERROR reading union field of type
    }
}
pub unsafe fn aggregate(u: &Agg) -> u8 {
    unsafe { *u.pair.1 } //~ ERROR reading union field of type `(usize, &'static u8)`
}
pub unsafe fn aggregate_int_part(u: &Agg) -> usize {
    unsafe { u.pair.0 } //~ ERROR reading union field of type `(usize, &'static u8)`
}
pub unsafe fn fn_ptr(u: &FnU) -> u32 {
    unsafe { (u.f)() } //~ ERROR reading union field of type `fn() -> u32`
}
pub unsafe fn nonnull(u: &NnU) -> NonNull<u8> {
    unsafe { u.nn } //~ ERROR reading union field of type `NonNull<u8>`
}
pub unsafe fn borrow(u: &PunSlice<'_>) -> usize {
    let r = unsafe { &u.typed }; //~ ERROR reading union field of type
    r.len()
}
pub unsafe fn generic<T: Copy>(g: &G<T>) -> T {
    unsafe { g.t } //~ ERROR reading union field of type `T`
}

// --- allowed ---
pub unsafe fn plain(p: &Plain) -> u64 { unsafe { p.a + p.b[0] as u64 + p.c as u64 } }
pub unsafe fn plain_of_ptr_union(u: &FnU) -> usize { unsafe { u.n } }
pub fn write(u: &mut PunSlice<'static>, s: &'static [u64]) { u.typed = s; }
pub unsafe fn wildcard(u: &PunSlice<'_>) -> usize { let PunSlice { typed: _ } = *u; 0 }
pub fn raw_borrow(u: &PunSlice<'_>) -> *const *const [u64] { &raw const u.raw }
pub fn maybe_uninit(m: MaybeUninit<&'static u8>) -> &'static u8 { unsafe { m.assume_init() } }
