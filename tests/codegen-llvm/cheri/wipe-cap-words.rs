// M7.2a-w1 [LLM-generated]: a T-aware RawVec / Box final dealloc of a
// cap-containing buffer emits
//   call addrspace(200) void @llvm.mem.fact() [ "mem.wipe_cap_words"(ptr addrspace(200) <ptr>, i64 <len>) ]
// after element drop and before deallocate, with len equal to the
// returned Layout / capacity. Cap-free `Vec<u8>` gets no fact.
// `Vec<Rc<_>>`, `Vec<&str>`, and `Box<node with caps>` do.
// Gated by `-Zcheri-annotate` and `-Zcheri-wipe-cap-words` (default on;
// `no` emits nothing).
//
//@ revisions: on off
//@ compile-flags: -Copt-level=0 -Cdebuginfo=0
//@[off] compile-flags: -Zcheri-wipe-cap-words=no
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

// Facts live in monomorphized RawVec / Box drop, which rustc emits before
// the no_mangle wrappers at -Copt-level=0. Cap-free RawVec<u8> has no fact.
// on: RawVec<alloc::rc::Rc
// on: "mem.wipe_cap_words"
// on: RawVec<&str>
// on: "mem.wipe_cap_words"
// on: RawVec<u8>
// on-NOT: "mem.wipe_cap_words"
// on: Box<wipe_cap_words::CapNode>
// on: "mem.wipe_cap_words"
// off-NOT: "mem.wipe_cap_words"
