//@ no-prefer-dynamic
//@ compile-flags: -Ccodegen-units=1
//! A bump allocator over a static arena that can resize the most recent block in place,
//! for the realloc tests in tests/ui/cheri/qemu. Tests pick the `#[global_allocator]`.
//! `Grow::wide()` hands back a capability derived from the arena (like most bare-metal
//! allocators); `Grow::narrow()` hands back the pointer it was given when it resizes in
//! place, so a bounded block keeps its old bounds.
//! LLM-generated research prototype (not for upstream).
#![no_std]
#![crate_type = "rlib"]
#![feature(stdarch_cheri)]

use core::alloc::{GlobalAlloc, Layout};
use core::arch::cheri;
use core::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub const ARENA_SIZE: usize = 1 << 20;

#[repr(C, align(4096))]
pub struct Arena(pub [u8; ARENA_SIZE]);
pub static mut ARENA: Arena = Arena([0; ARENA_SIZE]);
static NEXT: AtomicUsize = AtomicUsize::new(0);
static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Number of `realloc`s the inner allocator did in place.
pub static IN_PLACE: AtomicUsize = AtomicUsize::new(0);
/// Number of `realloc` calls the inner allocator received.
pub static REALLOCS: AtomicUsize = AtomicUsize::new(0);

pub struct Grow {
    narrow: bool,
}

impl Grow {
    pub const fn wide() -> Self {
        Grow { narrow: false }
    }
    pub const fn narrow() -> Self {
        Grow { narrow: true }
    }
}

pub fn root() -> *mut u8 {
    (&raw mut ARENA) as *mut u8
}

unsafe impl GlobalAlloc for Grow {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let start = cheri::address_get(root());
        let off = NEXT.load(Relaxed);
        let a = (start + off).next_multiple_of(l.align()) - start;
        if a + l.size() > ARENA_SIZE {
            return core::ptr::null_mut();
        }
        NEXT.store(a + l.size(), Relaxed);
        LAST.store(a, Relaxed);
        unsafe { root().add(a) }
    }
    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        REALLOCS.fetch_add(1, Relaxed);
        let off = cheri::address_get(p) - cheri::address_get(root());
        let in_place = if off == LAST.load(Relaxed) {
            if off + new > ARENA_SIZE {
                return core::ptr::null_mut();
            }
            NEXT.store(off + new, Relaxed);
            true
        } else {
            new <= l.size()
        };
        if in_place {
            IN_PLACE.fetch_add(1, Relaxed);
            return if self.narrow { p } else { unsafe { root().add(off) } };
        }
        unsafe {
            let n = self.alloc(Layout::from_size_align_unchecked(new, l.align()));
            if !n.is_null() {
                core::ptr::copy_nonoverlapping(p, n, l.size().min(new));
            }
            n
        }
    }
}

/// Bounds of a capability: (address, base, length, tag).
pub fn bounds<T: ?Sized>(p: *const T) -> (usize, usize, usize, bool) {
    let p = p as *const u8;
    (cheri::address_get(p), cheri::base_get(p), cheri::length_get(p), cheri::tag_get(p))
}

/// Assert `p` is tagged with bounds exactly `[address, address + CRRL(size))`.
pub fn assert_exact<T: ?Sized>(p: *const T, size: usize) {
    let (a, b, l, t) = bounds(p);
    assert!(t, "untagged");
    assert_eq!(b, a, "base != address");
    assert_eq!(l, cheri::round_representable_length(size), "length");
}
