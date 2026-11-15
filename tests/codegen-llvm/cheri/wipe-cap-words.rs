// M7.2a-w1 [LLM-generated]: a T-aware RawVec / Box final dealloc of a
// cap-containing buffer emits
//   call addrspace(200) void @llvm.mem.fact() [ "mem.wipe_cap_words"(ptr addrspace(200) <ptr>, i64 <len>) ]
// after element drop and before deallocate, with len equal to the
// returned Layout / capacity. Cap-free `Vec<u8>` gets no fact.
// `Vec<Rc<_>>`, `Vec<&str>`, and `Box<node with caps>` do.
// Gated by `-Zcheri-annotate` and `-Zcheri-wipe-cap-words` (default on;
// `no` emits nothing and const-evals typic to false, so typic T keeps
// alloc.grow/shrink). LLVM `-cheri-invalidate-wipe-cap-words=false` /
// `-cheri-invalidate=none` only drop the fact; they do not disable the
// rustc typic path. Ablation of "none"/off must pass this rustc flag too.
//
//@ revisions: on off offO2
//@ compile-flags: -Cdebuginfo=0
//@[on] compile-flags: -Copt-level=0
//@[off] compile-flags: -Copt-level=0 -Zcheri-wipe-cap-words=no
//@[offO2] compile-flags: -Copt-level=2 -Zcheri-wipe-cap-words=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;

pub struct CapNode {
    p: *const u8,
    n: u32,
}

#[no_mangle]
pub fn drop_vec_u8(v: Vec<u8>) {
    core::mem::drop(v);
}

#[no_mangle]
pub fn drop_vec_rc(v: Vec<Rc<u32>>) {
    core::mem::drop(v);
}

#[no_mangle]
pub fn drop_vec_str(v: Vec<&str>) {
    core::mem::drop(v);
}

#[no_mangle]
pub fn drop_box_node(b: Box<CapNode>) {
    core::mem::drop(b);
}

// M7.2a-w2: typic RawVec grow/shrink relocate (and shrink-to-0) emit the
// fact on the abandoned old buffer. Cap-free Vec<u8> still does not.
#[no_mangle]
pub fn grow_vec_str(v: &mut Vec<&str>) {
    v.reserve(32);
}

#[no_mangle]
pub fn grow_vec_u8(v: &mut Vec<u8>) {
    v.reserve(32);
}

#[no_mangle]
pub fn shrink0_vec_str(v: &mut Vec<&str>) {
    let n = v.len();
    v.truncate(0);
    v.shrink_to_fit();
    let _ = n;
}

// Facts live in monomorphized RawVec / Box drop and typic grow/shrink helpers.
// Unique definition comments (not drop_glue) keep windows order-stable.
// Cap-free RawVec<u8> helpers are still emitted at -Copt-level=0 (both
// typic branches) but must not get a fact. Flag-off emits nothing.
// on: <alloc::raw_vec::RawVec<&str>>::shrink_wipe
// on: "mem.wipe_cap_words"
// on: <alloc::raw_vec::RawVec<&str>>::finish_grow_wipe
// on: "mem.wipe_cap_words"
// on: <alloc::raw_vec::RawVec<u8>>::finish_grow_wipe
// on-NOT: "mem.wipe_cap_words"
// on: <alloc::raw_vec::RawVec<alloc::rc::Rc<u32>> as core::ops::drop::Drop>::drop
// on: "mem.wipe_cap_words"
// on: <alloc::raw_vec::RawVec<&str> as core::ops::drop::Drop>::drop
// on: "mem.wipe_cap_words"
// on: <alloc::raw_vec::RawVec<u8> as core::ops::drop::Drop>::drop
// on-NOT: "mem.wipe_cap_words"
// on: <alloc::boxed::Box<wipe_cap_words::CapNode> as core::ops::drop::Drop>::drop
// on: "mem.wipe_cap_words"
// off-NOT: "mem.wipe_cap_words"
// At -Copt-level=2 the typic branch const-evals to false and DCE removes
// the typic relocate helpers. (At -Copt-level=0 both branches may still
// be emitted, so this check is O2-only.)
// offO2-NOT: "mem.wipe_cap_words"
// offO2-NOT: finish_grow_wipe
// offO2-NOT: shrink_wipe
