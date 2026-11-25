// M7.3b [LLM-generated]: more `vacates` specs, and the `ring = R` key. With `ring`, the
// vacated logical range `[from, to)` of a ring buffer of `R` bytes is wiped as
// `[from % R, ..)` up to the end of the ring plus `[0, ..)` for the part that wraps:
// two `mem.wipe_cap_words` facts (the second is empty unless the range wraps).
// The library now also carries the spec on `VecDeque::truncate` (ring; `clear`,
// `retain`, `retain_mut` and `resize_with` shrink through it), `Vec::split_off`,
// `vec::Drain`'s `Drop`, and the B-tree's `bulk_steal_left` / `bulk_steal_right`
// (`steal_left` / `steal_right` call them) and `do_merge` (the parent's vacated KV slot).
// Library MIR is built with the library's flags, so those wipes are in both revisions;
// `-Zcheri-spec-vacates=no` only turns off the local spec.
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
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::ptr::NonNull;

// B-tree merge (via `BTreeMap::remove`): the parent's vacated value slot (`Box<u64>`).
// CHECK-LABEL: define {{.*}}BalancingContext{{.*}}8do_merge
// CHECK: mem.wipe_cap_words
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
// B-tree steals: the vacated value slots of the child that gave up elements.
// CHECK-LABEL: define {{.*}}BalancingContext{{.*}}15bulk_steal_left
// CHECK: mem.wipe_cap_words
// CHECK-LABEL: define {{.*}}BalancingContext{{.*}}16bulk_steal_right
// CHECK: mem.wipe_cap_words
// The library's `VecDeque::truncate` for `Box<u64>`: the same two wipes.
// CHECK-LABEL: define {{.*}}VecDeque{{.*}}BoxyEE8truncate
// CHECK: mem.wipe_cap_words
// CHECK: mem.wipe_cap_words
// CHECK: ret void
// CHECK-LABEL: define {{.*}}VecINtNtB{{.*}}BoxyEE9split_off
// CHECK: mem.wipe_cap_words
// CHECK-LABEL: define {{.*}}DrainINtNtB{{.*}}BoxyEE{{.*}}Drop4drop
// CHECK: mem.wipe_cap_words
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

#[no_mangle]
pub fn deque_truncate(d: &mut VecDeque<Box<u64>>, n: usize) {
    d.truncate(n)
}

#[no_mangle]
pub fn vec_split_off(v: &mut Vec<Box<u64>>, at: usize) -> Vec<Box<u64>> {
    v.split_off(at)
}

#[no_mangle]
pub fn vec_drain(v: &mut Vec<Box<u64>>) {
    v.drain(1..3);
}

#[no_mangle]
pub fn btree_remove(m: &mut BTreeMap<u64, Box<u64>>, k: u64) -> Option<Box<u64>> {
    m.remove(&k)
}
