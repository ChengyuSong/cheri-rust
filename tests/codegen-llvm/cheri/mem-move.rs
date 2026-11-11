// M7.1a [LLM-generated]: a move of Box or Vec out of a local memory place emits
// `call addrspace(200) void @llvm.mem.fact() [ "mem.move"(ptr addrspace(200) <slot>) ]`.
// The pointer is the source slot (alloca, or alloca plus a constant field offset).
// A scalar owner is loaded first, so the fact follows that load.
// A memory owner emits the fact from the memcpy that reads the slot into a
// different pointer, or after the call that was passed the slot itself.
// The fact is not emitted when the move operand is only evaluated: that nulled
// `vec![elem; n]` / `from_elem` before the copy into the live binding.
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
// on-NEXT: call addrspace(200) void @llvm.mem.fact() [ "mem.move"(ptr addrspace(200) [[SLOT]]) ]
// on: call {{.*}}@eat_box({{.*}}[[B]])
// off-LABEL: @move_box(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn move_box() {
    let mut b = Box::new(1u64);
    touch_box(&raw mut b);
    eat_box(b);
}

// Vec is BackendRepr::Memory. At -O0 the call argument is a copy of the local;
// the fact is the next instruction after that memcpy, naming the source, and
// only then is eat_vec called. A pass-through of the same alloca would instead
// put the fact after the call (the call is what reads the slot).
// on-LABEL: @move_vec(
// on: call addrspace(200) void @llvm.memcpy{{.*}}[[V:%[0-9A-Za-z._]+]], i64 48,
// on-NEXT: call addrspace(200) void @llvm.mem.fact() [ "mem.move"(ptr addrspace(200) [[V]]) ]
// on: call {{.*}}@eat_vec(
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
// on: call addrspace(200) void @llvm.mem.fact() [ "mem.move"(ptr addrspace(200)
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

// from_elem's temporary may already have a fact (after the call that read it).
// The later copy into eat_vec's argument is a different slot: that memcpy's
// source gets the next fact. Do not reject the earlier fact.
// on-LABEL: @move_repeat(
// on: @llvm.memcpy{{.*}}[[ARG:%[0-9A-Za-z._]+]],{{.*}}[[SRC:%[0-9A-Za-z._]+]], i64 48,
// on-NEXT: call addrspace(200) void @llvm.mem.fact() [ "mem.move"(ptr addrspace(200) [[SRC]]) ]
// on: call {{.*}}@eat_vec({{.*}}[[ARG]]
// off-LABEL: @move_repeat(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn move_repeat() {
    let c = core::hint::black_box(alloc::vec::from_elem(5u64, 4));
    eat_vec(c);
}

// Return place: from_elem's Vec is copied out. No fact may precede that copy.
// on-LABEL: @return_from_elem(
// on-NOT: mem.move
// on: call addrspace(200) void @llvm.memcpy
// on-NEXT: call addrspace(200) void @llvm.mem.fact() [ "mem.move"(
// off-LABEL: @return_from_elem(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn return_from_elem() -> Vec<u64> {
    alloc::vec::from_elem(5u64, 4)
}

// CString::spec_new_impl shape: fill a Vec, then move it out.
// on-LABEL: @move_buffer(
// on-NOT: mem.move
// on: call addrspace(200) void @llvm.memcpy
// on-NEXT: call addrspace(200) void @llvm.mem.fact() [ "mem.move"(
// off-LABEL: @move_buffer(
// off-NOT: mem.move
// off: ret
#[no_mangle]
pub fn move_buffer() -> Vec<u8> {
    let mut buffer = Vec::with_capacity(4);
    buffer.extend_from_slice(b"hi");
    buffer
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
