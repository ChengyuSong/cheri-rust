// `off` and `narrow` keep the software bounds check; `narrow` also narrows the capability
// (M1). `elide` keeps it only on the path where `csetboundsexact` cleared the tag (see
// `cap-bounds/`). `drop` is the unsound measurement baseline: the `Assert(BoundsCheck)` is
// not emitted and nothing is narrowed.
//
//@ revisions: off narrow elide drop
//@ compile-flags: -Copt-level=1
// M5r R3 (D23): CHECKs below see the late CHERI pass decisions [LLM-generated].
//@ compile-flags: -Cllvm-args=-cheri-bounds-lowering-in-opt
//@ [narrow] compile-flags: -Zcheri-cap-bounds-checks=narrow -Cllvm-args=-cheri-skip-narrow=false
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ [drop] compile-flags: -Zcheri-drop-bounds-checks
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

// CHECK-LABEL: @idx(
#[no_mangle]
pub fn idx(s: &[u8], i: usize) -> u8 {
    // off-NOT: llvm.cheri
    // off: icmp
    // off: panic_bounds_check
    // narrow: icmp
    // narrow: @llvm.cheri.cap.bounds.set.i64(
    // narrow: panic_bounds_check
    // elide: @llvm.cheri.cap.bounds.set.exact.i64(
    // elide: @llvm.cheri.cap.tag.get(
    // elide: cheri.slowcheck:
    // elide: icmp
    // elide: panic_bounds_check
    // drop-NOT: llvm.cheri
    // drop-NOT: icmp
    // drop-NOT: panic_bounds_check
    s[i]
}
