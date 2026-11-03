// realloc/grow/shrink of allocation-bounded heap blocks (liballoc's CHERI heap bounding
// over an allocator that can resize the last block in place and hands back arena-derived
// capabilities).
//  pass:          all cases below, with exact bounds checked after each.
//  drop_padding:  -Zcheri-drop-bounds-checks; after growing inside the padding, a read
//                 inside the padding works and a read just past it faults.
//  drop_shrink:   -Zcheri-drop-bounds-checks; after a same-alignment shrink, a read past
//                 the new padded length faults although the old block was larger.
// See README.md in this directory.
//@ revisions: pass drop_padding drop_shrink
//@ [pass] run-pass
//@ [drop_padding] run-crash
//@ [drop_shrink] run-crash
//@ [drop_padding] compile-flags: -Zcheri-drop-bounds-checks
//@ [drop_shrink] compile-flags: -Zcheri-drop-bounds-checks
//@ [drop_padding] check-run-results
//@ [drop_shrink] check-run-results
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ aux-build: cheri_heap_grow.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
#![no_std]
#![no_main]
#![feature(stdarch_cheri)]
#![allow(dead_code, unused_imports)]

extern crate alloc;
extern crate cheri_heap_grow;
extern crate cheri_rt;

use alloc::vec::Vec;
use cheri_heap_grow::{Grow, IN_PLACE, REALLOCS, assert_exact};
use core::arch::cheri::round_representable_length as crrl;
use core::hint::black_box;
use core::sync::atomic::Ordering::Relaxed;

#[global_allocator]
static GLOBAL: Grow = Grow::wide();

#[inline(never)]
fn get(a: &[u8], i: usize) -> u8 {
    a[i]
}

fn filled(n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        v.push(i as u8);
    }
    v
}

fn check_data(v: &[u8], n: usize) {
    for i in 0..n {
        assert_eq!(v[i], i as u8);
    }
}

/// Growth inside the padding: 4097 B pads to CRRL(4097) = 4104; 4100 fits.
fn grow_in_padding() -> Vec<u8> {
    let mut v = filled(4097);
    assert_eq!(crrl(4097), 4104);
    assert_exact(v.as_ptr(), 4097);
    let p = v.as_ptr();
    let r = REALLOCS.load(Relaxed);
    v.reserve_exact(3);
    assert_eq!(v.capacity(), 4100);
    assert_eq!(v.as_ptr(), p, "moved");
    assert_eq!(REALLOCS.load(Relaxed), r, "inner allocator called");
    assert_exact(v.as_ptr(), 4100);
    v.resize(4100, 0);
    check_data(&v, 4097);
    v
}

#[cfg(pass)]
fn grow_past_padding_in_place() {
    // 4104 -> 6000: same alignment (8), last block, so the inner allocator extends it.
    let mut v = filled(4097);
    let p = v.as_ptr();
    let n = IN_PLACE.load(Relaxed);
    v.reserve_exact(6000 - 4097);
    assert_eq!(v.as_ptr(), p, "moved");
    assert_eq!(IN_PLACE.load(Relaxed), n + 1);
    assert_exact(v.as_ptr(), 6000);
    v.resize(6000, 7);
    check_data(&v, 4097);
    assert_eq!(v[5999], 7);
}

#[cfg(pass)]
fn grow_past_padding_by_copy() {
    // Not the last block: the inner allocator must move it.
    let mut v = filled(4097);
    let blocker = filled(16);
    let p = v.as_ptr();
    v.reserve_exact(6000 - 4097);
    assert_ne!(v.as_ptr(), p, "not moved");
    assert_exact(v.as_ptr(), 6000);
    check_data(&v, 4097);
    black_box(blocker);
}

#[cfg(pass)]
fn grow_stricter_alignment() {
    // 4000 B (align 1) -> 8100 B needs 16-byte alignment: allocate, copy, free.
    let mut v = filled(4000);
    assert_exact(v.as_ptr(), 4000);
    let r = REALLOCS.load(Relaxed);
    v.reserve_exact(8100 - 4000);
    assert_eq!(REALLOCS.load(Relaxed), r, "inner realloc used");
    assert_eq!(v.as_ptr() as usize % 16, 0);
    assert_exact(v.as_ptr(), 8100);
    check_data(&v, 4000);
}

/// Same-alignment shrink: 6000 B -> 4100 B (both need 8-byte alignment).
fn shrink() -> Vec<u8> {
    let mut v = filled(6000);
    let blocker = filled(16);
    let p = v.as_ptr();
    v.truncate(4100);
    v.shrink_to_fit();
    assert_eq!(v.as_ptr(), p, "moved");
    assert_exact(v.as_ptr(), 4100);
    check_data(&v, 4100);
    black_box(blocker);
    v
}

#[cfg(pass)]
fn shrink_padding_only() {
    // 4100 -> 4098 keeps the padded length 4104: no call to the inner allocator.
    let mut v = filled(4100);
    let p = v.as_ptr();
    let r = REALLOCS.load(Relaxed);
    v.truncate(4098);
    v.shrink_to_fit();
    assert_eq!(v.as_ptr(), p);
    assert_eq!(REALLOCS.load(Relaxed), r);
    assert_exact(v.as_ptr(), 4104);
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    #[cfg(pass)]
    {
        black_box(grow_in_padding());
        grow_past_padding_in_place();
        grow_past_padding_by_copy();
        grow_stricter_alignment();
        black_box(shrink());
        shrink_padding_only();
    }
    #[cfg(drop_padding)]
    {
        let v = grow_in_padding();
        // Inside the padding (4100..4104): readable, no software check.
        black_box(get(black_box(&v), black_box(4103)));
        // Past the padding: CHERI length fault.
        black_box(get(black_box(&v), black_box(4104)));
    }
    #[cfg(drop_shrink)]
    {
        let v = shrink();
        black_box(get(black_box(&v), black_box(4103)));
        black_box(get(black_box(&v), black_box(4104)));
    }
}
