// M2: target-neutral annotations on CHERI bounds narrowing, and CheriSkipNarrow.
// `emit` disables the skip pass so the annotations rustc emits are visible:
// - safe-pointer params carry the string attribute "mem.alloc_inbounds"; raw pointers never do;
// - a narrow-mode per-access narrow carries !cheri.access_narrow, !mem.checked (a software
//   bounds check is retained) and !mem.alloc_inbounds (base is a safe, allocation-bounded ref);
// - `get_unchecked` gets nothing (no check, no claim);
// - a slice minted by `from_raw_parts` gets a M3.3 `!cheri.contract_narrow` conversion
//   (kept even with CheriSkipNarrow: the raw operand is not provenance-proven);
// - in `elide` mode the narrow is the sole check, so it never carries !mem.checked.
// `skip` (default) removes proven narrows; the software check stays.
//
//@ revisions: emit skip elide
//@ compile-flags: -Copt-level=2
// M5r R3 (D23): CHECKs below see the late CHERI pass decisions [LLM-generated].
//@ compile-flags: -Cllvm-args=-cheri-bounds-lowering-in-opt
//@ [emit] compile-flags: -Zcheri-cap-bounds-checks=narrow -Cllvm-args=-cheri-skip-narrow=false
//@ [skip] compile-flags: -Zcheri-cap-bounds-checks=narrow
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

// CHECK-LABEL: @idx(
// CHECK-SAME: "mem.alloc_inbounds" "mem.range"="1:1" %s.0
#[no_mangle]
pub fn idx(s: &[u8], i: usize) -> u8 {
    // emit: icmp
    // emit: @llvm.cheri.cap.bounds.set.i64({{.*}}!cheri.access_narrow
    // emit: panic_bounds_check
    // skip-NOT: llvm.cheri.cap.bounds.set
    // skip: icmp
    // skip: panic_bounds_check
    // elide-NOT: !mem.checked
    // elide: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}!cheri.access_narrow
    // elide-NOT: !mem.checked
    // elide: ret i8
    s[i]
}

// CHECK-LABEL: @arr(
// CHECK-SAME: "mem.alloc_inbounds" %a
#[no_mangle]
pub fn arr(a: &[u64; 4], i: usize) -> u64 {
    // emit: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 32), !cheri.access_narrow
    // skip-NOT: llvm.cheri.cap.bounds.set
    // skip: panic_bounds_check
    // elide: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 32), !cheri.access_narrow
    // elide-NOT: !mem.checked
    // elide: ret i64
    a[i]
}

// CHECK-LABEL: @unchecked(
#[no_mangle]
pub unsafe fn unchecked(s: &[u8], i: usize) -> u8 {
    // CHECK-NOT: llvm.cheri.cap.bounds.set
    // CHECK-NOT: !mem.checked
    // CHECK-NOT: !mem.alloc_inbounds
    // CHECK: ret i8
    unsafe { *s.get_unchecked(i) }
}

// CHECK-LABEL: @raw(
// CHECK-NOT: "mem.alloc_inbounds"
// CHECK-SAME: {
#[no_mangle]
pub unsafe fn raw(p: *const u8, n: usize, i: usize) -> u8 {
    // M3.3: from_raw_parts emits a contract_narrow (typed conversion). The raw
    // operand is not provenance-proven, so the narrow stays under skip too.
    // CHECK: @llvm.cheri.cap.bounds.set.i64({{.*}}!cheri.contract_narrow
    // CHECK: panic_bounds_check
    let s = unsafe { core::slice::from_raw_parts(p, n) };
    s[i]
}
