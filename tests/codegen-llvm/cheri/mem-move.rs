// M7.1a [LLM-generated]: a move of Box or Vec out of a local memory place emits
// `call void @llvm.mem.fact() [ "mem.move"(ptr <slot>) ]`.
// The pointer is the source slot (alloca, or alloca plus a constant field offset).
// A scalar owner is loaded first, so the fact follows that load and precedes the
// call that receives the register. Vec is a memory operand passed by move, so the
// fact follows the call: the callee still reads the slot.
// Gated by `-Zcheri-annotate` on CHERI targets, same as the loan facts.
//
//@ revisions: on off
//@ compile-flags: -Copt-level=0 -Cdebuginfo=0
//@[off] compile-flags: -Zcheri-annotate=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;

#[no_mangle]
#[inline(never)]
pub fn eat_box(b: Box<u64>) {
    core::mem::forget(b);
}

#[no_mangle]
#[inline(never)]
pub fn eat_vec(v: Vec<u64>) {
    core::mem::forget(v);
}

#[no_mangle]
#[inline(never)]
pub fn touch_box(p: *mut Box<u64>) {
    core::hint::black_box(p);
}

#[no_mangle]
#[inline(never)]
pub fn touch_slot(slot: &mut Option<Box<u64>>) {
    core::hint::black_box(slot);
}

// Box moved out of an address-taken local. The raw borrow keeps the local in an alloca.
// on-LABEL: @move_box(
// on: call {{.*}}@touch_box(
// on: [[B:%.*]] = load ptr addrspace(200), ptr addrspace(200) [[SLOT:%[0-9A-Za-z._]+]],
// on-NEXT: call void @llvm.mem.fact() [ "mem.move"(ptr addrspace(200) [[SLOT]]) ]
// on: call {{.*}}@eat_box(ptr addrspace(200) [[B]])
// off-LABEL: @move_box(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn move_box() {
    let mut b = Box::new(1u64);
    touch_box(&raw mut b);
    eat_box(b);
}

// Vec is BackendRepr::Memory, so the move operand aliases the alloca. The fact
// is the next instruction after the consuming call.
// on-LABEL: @move_vec(
// on: call {{.*}}@eat_vec({{.*}}[[V:%[0-9A-Za-z._]+]])
// on-NEXT: call void @llvm.mem.fact() [ "mem.move"(ptr addrspace(200) [[V]]) ]
// off-LABEL: @move_vec(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn move_vec() {
    let v = Vec::<u64>::new();
    eat_vec(v);
}

// The slot escaped as `&mut Option<Box<u64>>`. After that borrow ends, the Box
// (or the Option that holds it) is moved out of the same alloca.
// on-LABEL: @move_escaped_box(
// on: call {{.*}}@touch_slot(
// on: call void @llvm.mem.fact() [ "mem.move"(ptr addrspace(200)
// on: call {{.*}}@eat_box(
// off-LABEL: @move_escaped_box(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn move_escaped_box() {
    let mut slot = Some(Box::new(1u64));
    touch_slot(&mut slot);
    if let Some(b) = slot {
        eat_box(b);
    }
}

// Copy type: no move, no fact.
// on-LABEL: @copy_u64(
// on-NOT: mem.move
// on: ret
// off-LABEL: @copy_u64(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn copy_u64(x: u64) -> u64 {
    let y = x;
    y
}

// Raw pointer is Copy and is not an owner.
// on-LABEL: @copy_raw(
// on-NOT: mem.move
// on: ret
// off-LABEL: @copy_raw(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn copy_raw(p: *const u64) -> *const u64 {
    let q = p;
    q
}

// Reborrow is a reference, not a move of an owner.
// on-LABEL: @reborrow(
// on-NOT: mem.move
// on: ret
// off-LABEL: @reborrow(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn reborrow(r: &u64) -> &u64 {
    let s = &*r;
    s
}
