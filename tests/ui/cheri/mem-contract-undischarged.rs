//@ check-pass
// M3.5 [LLM-generated]: `mem_contract_undischarged` is warn-by-default.
// Flags unsafe raw→typed sites lacking a covering `requires` / `mem_contract`.
// Provenance does NOT silence this lint.
#![no_std]
#![crate_type = "lib"]
#![feature(contracts, contracts_internals, stmt_expr_attributes, proc_macro_hygiene)]
#![allow(internal_features)]

// --- uncovered ---
pub unsafe fn steal<'a>(p: *const u8, n: usize) -> &'a [u8] {
    core::slice::from_raw_parts(p, n)
    //~^ WARNING unsafe raw→typed conversion
}

pub unsafe fn steal_ref<'a>(p: *const u8) -> &'a u8 {
    &*p
    //~^ WARNING unsafe raw→typed conversion
}

// --- fn-level coverage ---
#[core::contracts::requires(core::contracts::valid_slice::<u8>(p, n))]
pub unsafe fn covered_fn<'a>(p: *const u8, n: usize) -> &'a [u8] {
    core::slice::from_raw_parts(p, n)
}

// --- block-level coverage ---
pub unsafe fn covered_block<'a>(p: *const u8, n: usize) -> &'a [u8] {
    #[core::contracts::requires(core::contracts::valid_slice::<u8>(p, n))]
    {
        core::slice::from_raw_parts(p, n)
    }
}

// --- mismatched predicate (ref does not cover slice) ---
#[core::contracts::requires(core::contracts::valid_ref::<u8>(p))]
pub unsafe fn mismatched<'a>(p: *const u8, n: usize) -> &'a [u8] {
    core::slice::from_raw_parts(p, n)
    //~^ WARNING unsafe raw→typed conversion
}

#[allow(mem_contract_undischarged)]
pub unsafe fn allowed<'a>(p: *const u8, n: usize) -> &'a [u8] {
    core::slice::from_raw_parts(p, n)
}

// --- M3.6: transmute raw→typed ---
pub unsafe fn steal_tm<'a>(p: *const u8, n: usize) -> &'a [u8] {
    let raw: *const [u8] = core::ptr::slice_from_raw_parts(p, n);
    core::mem::transmute(raw)
    //~^ WARNING unsafe raw→typed conversion
}

#[core::contracts::requires(core::contracts::valid_slice::<u8>(p, n))]
pub unsafe fn covered_tm<'a>(p: *const u8, n: usize) -> &'a [u8] {
    let raw: *const [u8] = core::ptr::slice_from_raw_parts(p, n);
    core::mem::transmute(raw)
}

pub unsafe fn steal_as_ref<'a>(p: *const u8) -> Option<&'a u8> {
    p.as_ref()
    //~^ WARNING unsafe raw→typed conversion
}
