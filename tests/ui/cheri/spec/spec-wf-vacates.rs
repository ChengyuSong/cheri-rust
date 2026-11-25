// [LLM-generated] M7.3: well-formedness of the `vacates(ty, block, from, to)` spec, the
// `old(..)` atom and `%` (checked in `check_attr` on every target).
#![feature(rustc_attrs)]
#![crate_type = "lib"]
#![allow(unused)]

use std::ptr::NonNull;

pub struct Node<K> {
    len: u16,
    keys: [K; 4],
}

pub struct Buf<T> {
    ptr: NonNull<T>,
    head: usize,
    len: usize,
    cap: usize,
    node: NonNull<Node<T>>,
}

impl<T> Buf<T> {
    // Well-formed specs: no error.
    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = self.len * size_of::<T>(), to = old(self.len) * size_of::<T>())"]
    pub fn ok_pop(&mut self) {}

    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = ((self.head + self.len) % self.cap) * size_of::<T>(), to = ((self.head + self.len) % self.cap + old(self.len) - self.len) * size_of::<T>())"]
    pub fn ok_ring(&mut self) {}

    #[rustc_cheri_spec = "vacates(ty = T, block = &self.node.keys, from = (old(self.node.len) - 1) * size_of::<T>(), to = old(self.node.len) * size_of::<T>())"]
    pub fn ok_addr(&mut self) {}

    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = self.len)"]
    //~^ ERROR malformed `rustc_cheri_spec`: `vacates` needs `to = ...`
    pub fn bad_missing(&mut self) {}

    #[rustc_cheri_spec = "vacates(ty = T, block = self.len, from = 0, to = 8)"]
    //~^ ERROR invalid `rustc_cheri_spec`: `block = self.len`: `usize` is not a pointer
    pub fn bad_block(&mut self) {}

    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = self.nope, to = 8)"]
    //~^ ERROR invalid `rustc_cheri_spec`: no field `nope`
    pub fn bad_field(&mut self) {}

    #[rustc_cheri_spec = "frees(block = self.ptr, size = old(self.cap))"]
    //~^ ERROR malformed `rustc_cheri_spec`: `old(..)` is only allowed in `vacates`, not in `frees`
    pub fn bad_old(&mut self) {}

    #[rustc_cheri_spec = "frees(block = &self.ptr, size = self.cap)"]
    //~^ ERROR malformed `rustc_cheri_spec`: expected an identifier
    pub fn bad_addr(&mut self) {}
}
