// [LLM-generated] M7.3b: well-formedness of the `ring = expr` key of `vacates` (a vacated
// range of a ring buffer that may wrap; checked in `check_attr` on every target).
#![feature(rustc_attrs)]
#![crate_type = "lib"]
#![allow(unused)]

use std::ptr::NonNull;

pub struct Ring<T> {
    ptr: NonNull<T>,
    head: usize,
    len: usize,
    cap: usize,
}

impl<T> Ring<T> {
    // Well-formed: no error.
    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = (old(self.head) + self.len) * size_of::<T>(), to = (old(self.head) + old(self.len)) * size_of::<T>(), ring = self.cap * size_of::<T>())"]
    pub fn ok_truncate(&mut self) {}

    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = 0, to = 8, ring = old(self.cap) * size_of::<T>())"]
    pub fn ok_old_ring(&mut self) {}

    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = 0, to = 8, ring = self.nope)"]
    //~^ ERROR invalid `rustc_cheri_spec`: no field `nope`
    pub fn bad_ring_field(&mut self) {}

    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = 0, to = 8, ring = 4, ring = 8)"]
    //~^ ERROR malformed `rustc_cheri_spec`: duplicate key `ring` in `vacates`
    pub fn bad_ring_dup(&mut self) {}

    #[rustc_cheri_spec = "frees(block = self.ptr, size = self.cap, ring = 8)"]
    //~^ ERROR malformed `rustc_cheri_spec`: unknown key `ring` in `frees`
    pub fn bad_ring_frees(&mut self) {}
}
