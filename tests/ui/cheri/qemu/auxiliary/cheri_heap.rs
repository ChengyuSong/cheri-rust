//@ no-prefer-dynamic
//@ compile-flags: -Ccodegen-units=1
//! A bump `#[global_allocator]` over a static arena, for heap tests in tests/ui/cheri/qemu.
//! It derives every block from the arena capability (1 MiB bounds) and never re-bounds it,
//! like most bare-metal allocators. Allocation-level bounds come from liballoc
//! (`alloc::alloc::alloc` pads, aligns and `CSetBoundsExact`s each block on CHERI purecap).
//! LLM-generated research prototype (not for upstream).
#![no_std]
#![crate_type = "rlib"]
#![feature(stdarch_cheri)]

use core::alloc::{GlobalAlloc, Layout};
use core::arch::cheri;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const ARENA_SIZE: usize = 1 << 20;

#[repr(C, align(4096))]
pub struct Arena(pub [u8; ARENA_SIZE]);
pub static mut ARENA: Arena = Arena([0; ARENA_SIZE]);
static NEXT: AtomicUsize = AtomicUsize::new(0);

// Not a ZST: a capability to a zero-sized static has no size in __cap_relocs, and ld.lld
// warns "could not determine size of cap reloc" for the `&GLOBAL` the allocator shim takes.
pub struct Bump(#[allow(dead_code)] u8);

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let base = (&raw mut ARENA) as *mut u8;
        let start = cheri::address_get(base);
        let mut off = NEXT.load(Ordering::Relaxed);
        loop {
            let a = (start + off).next_multiple_of(l.align()) - start;
            let end = a + l.size();
            if end > ARENA_SIZE {
                return core::ptr::null_mut();
            }
            match NEXT.compare_exchange(off, end, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return unsafe { base.add(a) },
                Err(o) => off = o,
            }
        }
    }
    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}
}

#[global_allocator]
static GLOBAL: Bump = Bump(0);

/// Bounds of a capability: (address, base, length, tag).
pub fn bounds<T: ?Sized>(p: *const T) -> (usize, usize, usize, bool) {
    let p = p as *const u8;
    (cheri::address_get(p), cheri::base_get(p), cheri::length_get(p), cheri::tag_get(p))
}
