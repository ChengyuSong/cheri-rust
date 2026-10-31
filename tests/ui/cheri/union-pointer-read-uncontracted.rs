//@ check-pass
//@ only-riscv64cheri-unknown-none-purecap
//@ compile-flags: -Zcheri-contract-narrow=no
// M4.0 [LLM-generated]: "contracted code" = CHERI target with -Zcheri-contract-narrow
// (default on). With the contract flags off, pointer-carrying union reads are not
// rejected (plain Rust semantics; no CHERI contract guarantees are claimed).
#![no_std]
#![crate_type = "lib"]
pub union PunSlice<'a> { pub raw: *const [u64], pub typed: &'a [u64] }
pub unsafe fn bu<'a>(p: *const u64, n: usize) -> &'a [u64] {
    let u = PunSlice { raw: core::ptr::slice_from_raw_parts(p, n) };
    unsafe { u.typed }
}
