// [LLM-generated] M7.2e: well-formedness errors for `live` statement specs (resolved
// when the MIR is built, on every target, also when no CHERI code is emitted).
#![feature(rustc_attrs)]
#![crate_type = "lib"]
#![allow(unused)]

use std::alloc::Layout;

#[rustc_cheri_spec = "relocates(block = ptr, size = old.size, keep = new.size)"]
pub unsafe fn reloc(ptr: *mut u8, old: Layout, new: Layout) -> *mut u8 { ptr }

pub unsafe fn plain(ptr: *mut u8) -> *mut u8 { ptr }

pub unsafe fn ok(ptr: *mut u8, n: usize, old: Layout, new: Layout) {
    #[rustc_cheri_spec = "live(block = ptr, bytes = n * 8, held = *ptr)"]
    let r = reloc(ptr, old, new);
}

pub unsafe fn no_local(ptr: *mut u8, old: Layout, new: Layout) {
    #[rustc_cheri_spec = "live(block = nope, bytes = 0, held = *ptr)"]
    //~^ ERROR invalid `rustc_cheri_spec`: no local variable named `nope`
    let r = reloc(ptr, old, new);
}

pub unsafe fn not_reloc(ptr: *mut u8) {
    #[rustc_cheri_spec = "live(block = ptr, bytes = 0, held = *ptr)"]
    //~^ ERROR invalid `rustc_cheri_spec`: `live`: `plain` has no `relocates` spec
    let r = plain(ptr);
}

pub unsafe fn not_call(ptr: *mut u8) {
    #[rustc_cheri_spec = "live(block = ptr, bytes = 0, held = *ptr)"]
    //~^ ERROR invalid `rustc_cheri_spec`: `live` must be on a `let` whose initializer is a call
    let r = ptr;
}

pub unsafe fn bad_bytes(ptr: *mut u8, old: Layout, new: Layout) {
    #[rustc_cheri_spec = "live(block = ptr, bytes = ptr, held = *ptr)"]
    //~^ ERROR invalid `rustc_cheri_spec`: `ptr` has type `*mut u8`, not an integer
    let r = reloc(ptr, old, new);
}

pub unsafe fn bad_block(n: usize, ptr: *mut u8, old: Layout, new: Layout) {
    #[rustc_cheri_spec = "live(block = n, bytes = 0, held = *ptr)"]
    //~^ ERROR invalid `rustc_cheri_spec`: `block`: `usize` is not a pointer
    let r = reloc(ptr, old, new);
}
