// Allocation-level bounds on CHERI purecap: every heap block (Box, Vec, String, Rc, after
// growth and shrinking), escaping stack array and static array is a capability bounded to
// its allocation, padded to an exactly representable length and aligned for it.
// See README.md in this directory.
//@ run-pass
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ aux-build: cheri_heap.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
#![no_std]
#![no_main]
#![feature(stdarch_cheri)]

extern crate alloc;
extern crate cheri_heap;
extern crate cheri_rt;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::cheri;
use core::hint::black_box;

#[track_caller]
fn check(what: &str, p: *const u8, size: usize) {
    let (addr, base, len, tag) = cheri_heap::bounds(p);
    let want = cheri::round_representable_length(size);
    let align = (!cheri::representable_alignment_mask(want)).wrapping_add(1);
    if !tag || base != addr || len != want || addr % align != 0 {
        cheri_rt::println!(
            "{what}: size {size:#x}: addr {addr:#x} base {base:#x} len {len:#x} tag {tag} \
             (want len {want:#x}, align {align})"
        );
        panic!("bad bounds");
    }
}

static S32: [u64; 4] = [1, 2, 3, 4];
static mut S5000: [u8; 5000] = [0; 5000];
static mut S1FF9: [u8; 0x1ff9] = [0; 0x1ff9];
static mut S3FF8: [u8; 0x3ff8] = [0; 0x3ff8];

#[inline(never)]
fn stack<const N: usize>() {
    let a = [7u8; N];
    check("stack", black_box(&a).as_ptr(), N);
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    let b = Box::new([1u64; 4]);
    check("Box<[u64; 4]>", (&*b as *const [u64; 4]).cast(), 32);
    for n in [1usize, 3, 16, 100, 4095, 4096, 4097, 5000, 0x1ff9, 0x3ff1, 0x3ff8, 70000] {
        let v: Vec<u8> = alloc::vec![0u8; n];
        check("vec![0u8; n]", v.as_ptr(), n);
        let z: Vec<u8> = Vec::with_capacity(n);
        check("Vec::with_capacity", z.as_ptr(), n);
    }
    // Growth: every reallocation gets fresh exact bounds.
    let mut v: Vec<u32> = Vec::new();
    for i in 0..3000u32 {
        v.push(i);
        check("Vec<u32> push", v.as_ptr().cast(), v.capacity() * 4);
    }
    assert_eq!(v.iter().map(|&x| x as u64).sum::<u64>(), 2999 * 3000 / 2);
    // Shrinking.
    v.truncate(10);
    v.shrink_to_fit();
    check("shrink_to_fit", v.as_ptr().cast(), 40);
    assert_eq!(v[9], 9);
    let bs: Box<[u32]> = v.into_boxed_slice();
    check("into_boxed_slice", bs.as_ptr().cast(), 40);
    let mut s = String::new();
    for _ in 0..100 {
        s.push_str("cheri");
    }
    check("String", s.as_ptr(), s.capacity());
    let rc = Rc::new([0u8; 24]);
    // The Rc allocation holds the two counts in front of the value; the value's
    // capability is derived from the allocation's.
    let (_, base, len, tag) = cheri_heap::bounds(Rc::as_ptr(&rc));
    assert!(tag && len == 16 + 24 && base + 16 == cheri::address_get(Rc::as_ptr(&rc)));

    stack::<32>();
    stack::<5000>();
    stack::<0x1ff9>();
    stack::<0x3ff8>();
    check("static [u64; 4]", (&raw const S32).cast(), 32);
    check("static [u8; 5000]", (&raw const S5000).cast(), 5000);
    check("static [u8; 0x1ff9]", (&raw const S1FF9).cast(), 0x1ff9);
    check("static [u8; 0x3ff8]", (&raw const S3FF8).cast(), 0x3ff8);
}
