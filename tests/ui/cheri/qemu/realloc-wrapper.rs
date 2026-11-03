// `CheriBoundedAlloc` over an allocator that, when it resizes in place, hands back the
// (bounded) pointer it was given. Bounds can only shrink, so growing in place needs a
// wider capability:
//  root:   `with_root` re-derives it from the heap root -> the block stays in place.
//  noroot: no wider capability -> liballoc moves the block (allocate, copy, free).
// Both end with exact bounds. See README.md in this directory.
//@ revisions: root noroot
//@ run-pass
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ aux-build: cheri_heap_grow.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
#![no_std]
#![no_main]
#![feature(cheri_bounded_alloc)]

extern crate alloc;
extern crate cheri_heap_grow;
extern crate cheri_rt;

use alloc::alloc::CheriBoundedAlloc;
use alloc::vec::Vec;
use cheri_heap_grow::{Grow, IN_PLACE, assert_exact};
use core::sync::atomic::Ordering::Relaxed;

#[cfg(root)]
#[global_allocator]
static GLOBAL: CheriBoundedAlloc<Grow> =
    unsafe { CheriBoundedAlloc::with_root(Grow::narrow(), (&raw mut cheri_heap_grow::ARENA).cast()) };
#[cfg(noroot)]
#[global_allocator]
static GLOBAL: CheriBoundedAlloc<Grow> = CheriBoundedAlloc::new(Grow::narrow());

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let mut v: Vec<u8> = Vec::with_capacity(4097);
    for i in 0..4097 {
        v.push(i as u8);
    }
    let p = v.as_ptr();
    let n = IN_PLACE.load(Relaxed);
    v.reserve_exact(6000 - 4097);
    // The inner allocator resized in place either way.
    assert!(IN_PLACE.load(Relaxed) > n);
    #[cfg(root)]
    assert_eq!(v.as_ptr(), p, "moved");
    #[cfg(noroot)]
    assert_ne!(v.as_ptr(), p, "kept a capability narrower than the block");
    assert_exact(v.as_ptr(), 6000);
    for i in 0..4097 {
        assert_eq!(v[i], i as u8);
    }
    v.resize(6000, 1);
    assert_eq!(v[5999], 1);
}
