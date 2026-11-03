//! Unstable module containing the unstable contracts lang items and attribute macros.

pub use crate::macros::builtin::{contracts_ensures as ensures, contracts_requires as requires};
/// CHERI M3.2 (local prototype): the type-contract attribute for declarations.
pub use crate::macros::builtin::contracts_mem_contract as mem_contract;
pub use self::mem_contracts::{
    valid_box, valid_cstr, valid_ref, valid_ref_mut, valid_slice, valid_slice_mut,
};

/// CHERI M3.2 (local research prototype, LLM-generated): the Rust type-contract
/// vocabulary. Each predicate states "these raw parts form a valid value of Rust
/// type X". Its spatial part (CHERI-enforceable) is what the bodies check at run
/// time under `-Zcontract-checks`; the trusted part (initialised, live, unaliased,
/// ownership) is documented only. In `requires` / `mem_contract` they are always
/// lowered to IR facts, independent of `-Zcontract-checks`.
#[unstable(feature = "contracts", issue = "128044")]
pub mod mem_contracts {
    use crate::ffi::c_char;
    use crate::mem::{align_of, size_of};

    #[inline]
    fn aligned<T>(p: *const T) -> bool {
        p.addr() % align_of::<T>() == 0
    }
    #[inline]
    fn fits<T>(len: usize) -> bool {
        len.checked_mul(size_of::<T>()).is_some_and(|b| b <= isize::MAX as usize)
    }

    /// `&'a [T]` from `(p, len)`. Spatial: `[p, p+len*size_of T)` in one allocation,
    /// aligned, non-null, `len*size_of T <= isize::MAX`. Trusted: initialised, live
    /// for `'a`, not mutated while shared.
    #[unstable(feature = "contracts", issue = "128044")]
    #[rustc_diagnostic_item = "mem_valid_slice"]
    pub fn valid_slice<T>(p: *const T, len: usize) -> bool {
        !p.is_null() && aligned(p) && fits::<T>(len)
    }
    /// `&'a mut [T]` from `(p, len)`: `valid_slice` + unaliased.
    #[unstable(feature = "contracts", issue = "128044")]
    #[rustc_diagnostic_item = "mem_valid_slice_mut"]
    pub fn valid_slice_mut<T>(p: *mut T, len: usize) -> bool {
        valid_slice(p.cast_const(), len)
    }
    /// `&'a T`. Spatial: `[p, p+size_of T)` in one allocation, aligned, non-null.
    #[unstable(feature = "contracts", issue = "128044")]
    #[rustc_diagnostic_item = "mem_valid_ref"]
    pub fn valid_ref<T>(p: *const T) -> bool {
        !p.is_null() && aligned(p)
    }
    /// `&'a mut T`: `valid_ref` + unaliased.
    #[unstable(feature = "contracts", issue = "128044")]
    #[rustc_diagnostic_item = "mem_valid_ref_mut"]
    pub fn valid_ref_mut<T>(p: *mut T) -> bool {
        valid_ref(p.cast_const())
    }
    /// `Box<T>` (`Global`): as `valid_ref`; trusted: from `__rust_alloc` with
    /// `Layout::new::<T>()`, ownership transfers.
    #[unstable(feature = "contracts", issue = "128044")]
    #[rustc_diagnostic_item = "mem_valid_box"]
    pub fn valid_box<T>(p: *mut T) -> bool {
        valid_ref(p.cast_const())
    }
    /// `&'a CStr` as `*const c_char`. Spatial: non-null, a NUL within p's allocation.
    #[unstable(feature = "contracts", issue = "128044")]
    #[rustc_diagnostic_item = "mem_valid_cstr"]
    pub fn valid_cstr(p: *const c_char) -> bool {
        !p.is_null()
    }

    /// CHERI M3.5: marker at a block/statement `requires` site.
    /// Kept as a real call so LLVM IR shows the canon string (same vocabulary
    /// as function-level `#[rustc_mem_contract]`). Never discharges a narrow.
    #[inline(never)]
    #[cold]
    #[unstable(feature = "contracts", issue = "128044")]
    pub fn mem_contract_site(canon: &'static str) {
        unsafe {
            core::ptr::read_volatile(&raw const canon);
        }
    }
}

/// This is an identity function used as part of the desugaring of the `#[ensures]` attribute.
///
/// This is an existing hack to allow users to omit the type of the return value in their ensures
/// attribute.
///
/// Ideally, rustc should be able to generate the type annotation.
/// The existing lowering logic makes it rather hard to add the explicit type annotation,
/// while the function call is fairly straight forward.
#[unstable(feature = "contracts_internals", issue = "128044" /* compiler-team#759 */)]
// Similar to `contract_check_requires`, we need to use the user-facing
// `contracts` feature rather than the perma-unstable `contracts_internals`.
// Const-checking doesn't honor allow_internal_unstable logic used by contract expansion.
#[rustc_const_unstable(feature = "contracts", issue = "128044")]
#[lang = "contract_build_check_ensures"]
pub const fn build_check_ensures<Ret, C>(cond: C) -> C
where
    C: Fn(&Ret) -> bool + Copy + 'static,
{
    cond
}
