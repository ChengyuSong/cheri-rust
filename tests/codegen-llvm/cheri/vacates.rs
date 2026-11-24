// M7.3 [LLM-generated]: `#[rustc_cheri_spec = "vacates(ty, block, from, to)"]` on a
// function makes MIR building copy the block pointer and the `old(..)` operands on
// entry and emit a `WipeDead` before each return, which codegen lowers to
// `mem.wipe_cap_words(block + from, to - from)` when `ty` may hold capabilities.
// The library's Vec / VecDeque / BTree removal methods carry the spec, so their
// monomorphic copies in this crate (MIR built by the library) wipe the vacated slots
// past `len` for capability-holding element types and emit nothing for plain data.
// `-Zcheri-spec-vacates=no` turns off the lowering for MIR built in this crate.
//
//@ revisions: on off
//@ compile-flags: -Copt-level=0 -Cdebuginfo=0
//@[off] compile-flags: -Zcheri-spec-vacates=no
//@ only-riscv64cheri-unknown-none-purecap

#![feature(rustc_attrs)]
#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::NonNull;

// The library's `Vec::pop` (MIR built with the library's flags, default on) instantiated
// here for `Box<u64>`: wipes the vacated slot in both revisions.
// CHECK-LABEL: define {{.*}}VecINtNtB{{.*}}BoxyEE3pop
// CHECK: mem.wipe_cap_words
// CHECK: ret
// The local spec, `Stack<Box<u64>>`: the block pointer and `old(self.len)` are read on
// entry and `self.len` on return; the wipe covers `[len * 16, old_len * 16)`.
// on-LABEL: define {{.*}}StackINtNtCs{{.*}}BoxyEE9drop_last
// on: mem.wipe_cap_words
// on: ret void
// off-LABEL: define {{.*}}StackINtNtCs{{.*}}BoxyEE9drop_last
// off-NOT: mem.wipe_cap_words
// off: ret void
// Plain data (`Stack<u64>`): no capability, so no fact in either revision.
// CHECK-LABEL: define {{.*}}StackyE9drop_last
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret void
#[allow(dead_code)]
pub struct Stack<T> {
    ptr: NonNull<T>,
    len: usize,
}

impl<T> Stack<T> {
    #[inline(never)]
    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = self.len * size_of::<T>(), to = old(self.len) * size_of::<T>())"]
    pub fn drop_last(&mut self) {
        if self.len > 0 {
            self.len -= 1;
        }
    }
}

#[no_mangle]
pub fn local_spec(s: &mut Stack<Box<u64>>) {
    s.drop_last();
}

#[no_mangle]
pub fn local_spec_plain(s: &mut Stack<u64>) {
    s.drop_last();
}

#[no_mangle]
pub fn vec_pop(v: &mut Vec<Box<u64>>) -> Option<Box<u64>> {
    v.pop()
}
