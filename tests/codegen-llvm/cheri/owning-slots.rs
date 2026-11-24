// M7.3 [LLM-generated]: a memory local whose type can only hold owning heap pointers
// (global allocator), pointers to statics or function pointers gets `!mem.owning` on
// its alloca. Derived from the type: `Box<T>`, `Vec<T>`, `String`, `Rc<T>`, enums and
// structs of those (a lisp-like `V`), function pointers. Not marked: anything that may
// hold a reference or raw pointer (it may point into a frame), unions (`MaybeUninit`),
// containers with a non-global allocator, and slots with no capability at all.
// The late CHERI invalidate pass skips its scope-end range check for marked slots.
// Gated by `-Zcheri-annotate` and `-Zcheri-owning-slots` (default on).
//
//@ revisions: on off noann
//@ compile-flags: -Copt-level=0 -Cdebuginfo=0 -Zfewer-names=no
//@[off] compile-flags: -Zcheri-owning-slots=no
//@[noann] compile-flags: -Zcheri-annotate=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::MaybeUninit;

pub enum V {
    Nil,
    Int(i64),
    Sym(Rc<str>),
    Cons(Box<V>, Box<V>),
    List(Vec<V>),
    Shared(Rc<V>),
    Prim(fn(&[V]) -> V),
}

pub struct Holder<'a> {
    pub name: String,
    pub r: &'a u64,
}

#[inline(never)]
fn touch<T>(p: *const T) {
    core::hint::black_box(p);
}

// on-LABEL: @owning_marked(
// on-DAG: %b = alloca {{.*}}!mem.owning
// on-DAG: %v = alloca {{.*}}!mem.owning
// on-DAG: %s = alloca {{.*}}!mem.owning
// on-DAG: %lisp = alloca {{.*}}!mem.owning
// on-DAG: %f = alloca {{.*}}!mem.owning
// on-DAG: %pair = alloca {{.*}}!mem.owning
// on-DAG: %vr = alloca {{.*}}!mem.owning
// on: ret void
// off-LABEL: @owning_marked(
// off-NOT: mem.owning
// off: ret void
// noann-LABEL: @owning_marked(
// noann-NOT: mem.owning
// noann: ret void
#[no_mangle]
pub fn owning_marked(x: u64) {
    let b = Box::new(x);
    touch(&raw const b);
    let v: Vec<u64> = Vec::new();
    touch(&raw const v);
    let s = String::new();
    touch(&raw const s);
    let lisp = V::Int(x as i64);
    touch(&raw const lisp);
    let f: fn(u64) -> u64 = id;
    touch(&raw const f);
    let pair = (Box::new(1u8), x);
    touch(&raw const pair);
    // The slot holds the buffer pointer, length and capacity; the `&u64`s are in the heap.
    let vr: Vec<&u64> = Vec::new();
    touch(&raw const vr);
}

fn id(x: u64) -> u64 {
    x
}

// on-LABEL: @owning_not_marked(
// on-NOT: mem.owning
// on: ret void
#[no_mangle]
pub fn owning_not_marked(x: u64, h: Holder<'_>) {
    let r = &x;
    touch(&raw const r);
    let p = &raw const x as *const u8;
    touch(&raw const p);
    let mu: MaybeUninit<Box<u64>> = MaybeUninit::uninit();
    touch(&raw const mu);
    let o: Option<&u64> = Some(&x);
    touch(&raw const o);
    let n = x + 1;
    touch(&raw const n);
    touch(&raw const h);
}
