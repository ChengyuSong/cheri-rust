// `alloc::alloc::CheriBoundedAlloc` with an allocator that keeps a header in front of
// each block. liballoc hands the *bounded* block capability back to `dealloc`, so the
// header is out of bounds:
//  noroot: `CheriBoundedAlloc::new` passes it through -> the header read takes a CHERI
//          length fault.
//  root:   `CheriBoundedAlloc::with_root` re-derives the allocator's capability from the
//          heap root -> runs to completion, and blocks still have exact bounds.
// See README.md in this directory.
//@ revisions: root noroot
//@ [root] run-pass
//@ [noroot] run-crash
//@ [noroot] check-run-results
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
#![no_std]
#![no_main]
#![feature(stdarch_cheri, cheri_bounded_alloc)]

extern crate alloc;
extern crate cheri_rt;

use alloc::alloc::CheriBoundedAlloc;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::arch::cheri;
use core::cell::UnsafeCell;
use core::hint::black_box;

const HEAP: usize = 1 << 18;
const HDR: usize = 16;
#[repr(C, align(4096))]
struct Heap([u8; HEAP]);
static mut MEM: Heap = Heap([0; HEAP]);

/// Bump allocator with a 16-byte header (size, magic) in front of every block.
struct Headered {
    next: UnsafeCell<usize>,
}
unsafe impl Sync for Headered {}

unsafe impl GlobalAlloc for Headered {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe {
            let base = (&raw mut MEM) as *mut u8;
            let start = cheri::address_get(base);
            let align = l.align().max(HDR);
            let a = (start + *self.next.get() + HDR).next_multiple_of(align) - start;
            if a + l.size() > HEAP {
                return core::ptr::null_mut();
            }
            *self.next.get() = a + l.size();
            let p = base.add(a);
            (p.sub(HDR) as *mut [usize; 2]).write([l.size(), 0xa110c]);
            p
        }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe {
            let h = p.sub(HDR) as *mut [usize; 2];
            let [size, magic] = h.read();
            if size != l.size() || magic != 0xa110c {
                panic!("bad header");
            }
            h.write([size, 0xf4ee]);
        }
    }
}

#[cfg(root)]
#[global_allocator]
static GLOBAL: CheriBoundedAlloc<Headered> = unsafe {
    CheriBoundedAlloc::with_root(Headered { next: UnsafeCell::new(0) }, (&raw mut MEM).cast())
};
#[cfg(noroot)]
#[global_allocator]
static GLOBAL: CheriBoundedAlloc<Headered> =
    CheriBoundedAlloc::new(Headered { next: UnsafeCell::new(0) });

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let mut v: Vec<u64> = Vec::new();
    for i in 0..1000 {
        v.push(i);
        let p = v.as_ptr() as *const u8;
        let want = cheri::round_representable_length(v.capacity() * 8);
        assert!(cheri::tag_get(p) && cheri::base_get(p) == cheri::address_get(p));
        assert_eq!(cheri::length_get(p), want);
    }
    assert_eq!(v.iter().sum::<u64>(), 999 * 1000 / 2);
    drop(black_box(v));
}
