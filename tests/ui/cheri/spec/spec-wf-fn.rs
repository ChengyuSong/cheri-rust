// [LLM-generated] M7.2e: well-formedness errors for `#[rustc_cheri_spec]` on functions
// and statements (checked in `check_attr` on every target, also when no CHERI code is
// emitted).
#![feature(rustc_attrs)]
#![crate_type = "lib"]
#![allow(unused)]

use std::alloc::Layout;
use std::ptr::NonNull;

pub struct Buf<T> {
    ptr: NonNull<T>,
    cap: usize,
}

// Well-formed specs: no error.
#[rustc_cheri_spec = "frees(block = ptr, size = layout.size)"]
pub unsafe fn ok_frees(ptr: *mut u8, layout: Layout) {}

#[rustc_cheri_spec = "relocates(block = ptr, size = old.size, keep = new.size)"]
pub unsafe fn ok_relocates(ptr: *mut u8, old: Layout, new: Layout) -> *mut u8 { ptr }

impl<T> Buf<T> {
    #[rustc_cheri_spec = "dealloc_of(ty = T, block = self.ptr, size = self.cap * size_of::<T>())"]
    pub fn ok_dealloc_of(&mut self) {}

    #[rustc_cheri_spec = "frees(block = self.nope, size = self.cap)"]
    //~^ ERROR invalid `rustc_cheri_spec`: no field `nope`
    pub fn bad_field(&mut self) {}

    #[rustc_cheri_spec = "dealloc_of(ty = U)"]
    //~^ ERROR invalid `rustc_cheri_spec`: `U` is not a generic type parameter or `Self` here
    pub fn bad_ty(&mut self) {}
}

#[rustc_cheri_spec = "frees(block = ptr"]
//~^ ERROR malformed `rustc_cheri_spec`: expected `,`
pub unsafe fn bad_syntax(ptr: *mut u8) {}

#[rustc_cheri_spec = "wipes(block = ptr, size = 8)"]
//~^ ERROR malformed `rustc_cheri_spec`: unknown spec `wipes`
pub unsafe fn bad_form(ptr: *mut u8) {}

#[rustc_cheri_spec = "dealloc_of(ty = T, or = T)"]
//~^ ERROR malformed `rustc_cheri_spec`: `or = ...` needs `fields`
pub unsafe fn bad_or<T>(ptr: *mut T) {}

#[rustc_cheri_spec = "frees(block = ptr, size = 8, colour = 1)"]
//~^ ERROR malformed `rustc_cheri_spec`: unknown key `colour` in `frees`
pub unsafe fn bad_key(ptr: *mut u8) {}

#[rustc_cheri_spec = "frees(ptr, size = 8)"]
//~^ ERROR malformed `rustc_cheri_spec`: unexpected `ptr` in `frees`
pub unsafe fn bad_positional(ptr: *mut u8) {}

#[rustc_cheri_spec = "frees(block = q, size = 8)"]
//~^ ERROR invalid `rustc_cheri_spec`: no parameter or local named `q`
pub unsafe fn bad_param(ptr: *mut u8) {}

#[rustc_cheri_spec = "frees(block = ptr, size = ptr)"]
//~^ ERROR invalid `rustc_cheri_spec`: `ptr` has type `*mut u8`, not an integer
pub unsafe fn bad_size(ptr: *mut u8) {}

#[rustc_cheri_spec = "frees(block = n, size = 8)"]
//~^ ERROR invalid `rustc_cheri_spec`: `block = n`: `usize` is not a pointer
pub unsafe fn bad_block(n: usize) {}

#[rustc_cheri_spec = "relocates(block = ptr, size = old.size, keep = new.size, alloc = allocate(new), free = deallocate(ptr, old))"]
//~^ ERROR invalid `rustc_cheri_spec`: `alloc = ...` / `free = ...` are only allowed on a trait method
pub unsafe fn bad_rewrite(ptr: *mut u8, old: Layout, new: Layout) -> *mut u8 { ptr }

#[rustc_cheri_spec = "live(block = ptr, bytes = 0, held = *ptr)"]
//~^ ERROR invalid `rustc_cheri_spec`: `live` is a statement spec
pub unsafe fn live_on_fn(ptr: *mut u8) {}

// A named type is not a spec type (only generic parameters, `Self` and `*place`).
#[rustc_cheri_spec = "dealloc_of(ty = u64)"]
//~^ ERROR invalid `rustc_cheri_spec`: `u64` is not a generic type parameter or `Self` here
pub unsafe fn named_ty(ptr: *mut u64) {}

pub unsafe fn stmt_not_live(ptr: *mut u8, old: Layout, new: Layout) {
    #[rustc_cheri_spec = "frees(block = ptr, size = 8)"]
    //~^ ERROR `rustc_cheri_spec`: `frees` is a function spec; only `live` may be put on a statement
    let r = ok_relocates(ptr, old, new);
}
