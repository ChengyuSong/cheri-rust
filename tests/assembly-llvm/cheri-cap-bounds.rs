// `a[i]` under `-Z cheri-cap-bounds-checks`: with `elide` the bounds check becomes a
// `csetboundsexact` and the hot path has no index compare.
//
//@ revisions: off elide
//@ assembly-output: emit-asm
//@ compile-flags: -Copt-level=2
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

// CHECK-LABEL: array_small:
#[no_mangle]
pub fn array_small(a: &[u32; 16], i: usize) -> u32 {
    // off: bltu
    // off: clw
    // off: panic_bounds_check

    // elide-NOT: bltu
    // elide-NOT: bgeu
    // elide: csetboundsexact
    // elide-NOT: bltu
    // elide-NOT: bgeu
    // elide: clw
    // elide-NEXT: cret
    // elide-NOT: panic_bounds_check
    a[i]
}

// CHECK-LABEL: slice_load:
#[no_mangle]
pub fn slice_load(s: &[u64], i: usize) -> u64 {
    // off: bgeu
    // off: cld
    // off: panic_bounds_check

    // elide-NOT: bgeu
    // M5r R3 (D23): checks csetbounds presence and the tag test, not the block schedule
    // (the late pass's two-version shape is laid out by the backend).
    // elide: csetboundsexact [[C:a[0-9]]], a0,
    // elide-NEXT: cgettag [[T:a[0-9]]], [[C]]
    // elide: csetbounds {{a[0-9]}}, a0,
    // elide: bgeu
    // elide: cld
    // elide-NEXT: cret
    // elide: panic_bounds_check
    s[i]
}
