// M7.3b [LLM-generated]: the `ring = R` key of `vacates` on a local ring buffer. With
// `ring`, the vacated logical range `[from, to)` of a ring buffer of `R` bytes is wiped
// as `[from % R, ..)` up to the end of the ring plus `[0, ..)` for the part that wraps:
// two `mem.wipe_cap_words` facts (the second is empty unless the range wraps).
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
use core::ptr::NonNull;

// The local ring spec: two wipes, the second from the start of the block.
// on-LABEL: define {{.*}}RingINtNtCs{{.*}}BoxyEE8truncate
// on: urem
// on: select
// on: mem.wipe_cap_words
// on: mem.wipe_cap_words"(ptr addrspace(200) %{{.*}}, i64
// on: ret void
// off-LABEL: define {{.*}}RingINtNtCs{{.*}}BoxyEE8truncate
// off-NOT: mem.wipe_cap_words
// off: ret void
// Plain data in a ring: no fact in either revision.
// CHECK-LABEL: define {{.*}}RingyE8truncate
// CHECK-NOT: mem.wipe_cap_words
// CHECK: ret void
#[allow(dead_code)]
pub struct Ring<T> {
    ptr: NonNull<T>,
    head: usize,
    len: usize,
    cap: usize,
}

impl<T> Ring<T> {
    #[inline(never)]
    #[rustc_cheri_spec = "vacates(ty = T, block = self.ptr, from = (old(self.head) + self.len) * size_of::<T>(), to = (old(self.head) + old(self.len)) * size_of::<T>(), ring = self.cap * size_of::<T>())"]
    pub fn truncate(&mut self, len: usize) {
        if len < self.len {
            self.len = len;
        }
    }
}

#[no_mangle]
pub fn ring_truncate(r: &mut Ring<Box<u64>>, n: usize) {
    r.truncate(n)
}

#[no_mangle]
pub fn ring_truncate_plain(r: &mut Ring<u64>, n: usize) {
    r.truncate(n)
}
