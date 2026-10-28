// `off` / `narrow` / `elide` keep today's software bounds check. `narrow` and `elide`
// are accepted and do not change codegen in M0. `drop` is the unsound measurement
// baseline: the `Assert(BoundsCheck)` is not emitted.
//
//@ revisions: off narrow elide drop
//@ compile-flags: -Copt-level=1
//@ [narrow] compile-flags: -Zcheri-cap-bounds-checks=narrow
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ [drop] compile-flags: -Zcheri-drop-bounds-checks
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

// CHECK-LABEL: @idx(
#[no_mangle]
pub fn idx(s: &[u8], i: usize) -> u8 {
    // off: icmp
    // off: panic_bounds_check
    // narrow: icmp
    // narrow: panic_bounds_check
    // elide: icmp
    // elide: panic_bounds_check
    // drop-NOT: icmp
    // drop-NOT: panic_bounds_check
    s[i]
}
