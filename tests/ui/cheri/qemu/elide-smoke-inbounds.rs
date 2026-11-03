// In-bounds half of the M1 QEMU smoke matrix: every (len, off) case, first/last/middle
// index, in both modes; must run to completion (exit 0). LLM-generated research prototype.
//@ run-pass
//@ only-riscv64cheri-unknown-none-purecap
//@ aux-build: cheri_rt.rs
//@ compile-flags: -C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld
//@ revisions: off elide
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
#![no_std]
#![no_main]
// QEMU functional test of elide/off indexing — not a SAFETY-lint UI test.
#![allow(unknown_lints, mem_contract_undischarged)]

extern crate cheri_rt;

use core::hint::black_box;

#[repr(C, align(4096))]
struct Buf([u8; 8192]);
static mut BUF: Buf = Buf([0; 8192]);

#[inline(never)]
fn get(s: &[u8], i: usize) -> u8 {
    s[i]
}

#[inline(never)]
fn set(s: &mut [u8], i: usize, v: u8) {
    s[i] = v;
}

#[unsafe(no_mangle)]
extern "C" fn cheri_test_main() {
    // `&raw mut` then `&mut *`: avoids `static_mut_refs` (edition 2024). The
    // resulting `&mut *raw` is a conversion the M3.5 SAFETY lint would warn on;
    // allowed above — this file tests elide/off indexing, not contracts.
    let buf = unsafe { &mut *(&raw mut BUF) };
    for (k, b) in buf.0.iter_mut().enumerate() {
        *b = k as u8;
    }
    for &(len, off) in black_box(&[(16usize, 0usize), (4096, 0), (4096, 1), (4097, 1), (5, 3), (8192, 0)]) {
        let s = &mut buf.0[off..off + len];
        for i in [0, len / 2, len - 1] {
            let i = black_box(i);
            assert_eq!(get(s, i), (off + i) as u8);
            set(s, i, 0xa5);
            assert_eq!(get(s, i), 0xa5);
            set(s, i, (off + i) as u8);
        }
    }
}
