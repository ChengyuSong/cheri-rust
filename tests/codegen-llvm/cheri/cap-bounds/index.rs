// `-Z cheri-cap-bounds-checks` on built-in `a[i]` (M1). `off` keeps today's code. `narrow`
// narrows the capability and keeps the software check. `elide` narrows and removes the check:
// fully when the bounds are statically exact, behind a `cgettag` fast path otherwise.
// Elided accesses use a plain GEP (no `inbounds`/`nuw`) and no `llvm.assume`.
//
//@ revisions: off narrow elide
//@ compile-flags: -Copt-level=2
// M5r R3 (D23): CHECKs below see the late CHERI pass decisions [LLM-generated].
//@ compile-flags: -Cllvm-args=-cheri-bounds-lowering-in-opt
//@ [narrow] compile-flags: -Zcheri-cap-bounds-checks=narrow -Cllvm-args=-cheri-skip-narrow=false
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

// 64 bytes: exact for any base, so `elide` drops the check entirely.
// CHECK-LABEL: @array_small(
#[no_mangle]
pub fn array_small(a: &[u32; 16], i: usize) -> u32 {
    // off-NOT: llvm.cheri.cap.bounds.set
    // off: icmp
    // off: getelementptr inbounds nuw
    // off: panic_bounds_check

    // narrow: icmp
    // narrow: @llvm.cheri.cap.bounds.set.exact.i64(ptr addrspace(200) {{.*}}%a, i64 64)
    // narrow: getelementptr inbounds nuw
    // narrow: panic_bounds_check

    // elide-NOT: icmp
    // elide-NOT: llvm.assume
    // elide: [[C:%.*]] = {{.*}}@llvm.cheri.cap.bounds.set.exact.i64(ptr addrspace(200) {{.*}}%a, i64 64)
    // elide-NEXT: [[P:%.*]] = getelementptr [4 x i8], ptr addrspace(200) [[C]], i64 %i
    // elide-NEXT: load i32, ptr addrspace(200) [[P]]
    // elide-NOT: panic_bounds_check
    // elide: ret i32
    a[i]
}

// A local array is narrowed the same way.
// CHECK-LABEL: @local_array(
#[no_mangle]
pub fn local_array(i: usize, x: u64) -> u64 {
    // M5r R3 (D23): GVN merges the `i ^ 1` and `i` checks (same predicate) into one panic
    // block whose marker no longer names a single site, so the late pass keeps it (a merged
    // check is never removed). M5.0 narrowed here; see RESULTS-m5r.3.md.
    // elide: icmp ult i64 %i, 8
    // elide: panic_bounds_check
    // off: panic_bounds_check
    let mut a = [x; 8];
    a[i ^ 1] = x + 1;
    a[i]
}

// Slice: the length is dynamic, so `elide` tests the tag of `csetboundsexact` and keeps the
// software check only on the inexact path. The fast path has no index compare.
// CHECK-LABEL: @slice_load(
#[no_mangle]
pub fn slice_load(s: &[u64], i: usize) -> u64 {
    // off-NOT: llvm.cheri
    // off: icmp ult i64 %i, %s.1
    // off: panic_bounds_check

    // narrow: icmp ult i64 %i, %s.1
    // narrow: @llvm.cheri.cap.bounds.set.i64(
    // narrow: getelementptr inbounds nuw
    // narrow: panic_bounds_check

    // elide-NOT: icmp
    // elide: [[E:%.*]] = {{.*}}@llvm.cheri.cap.bounds.set.exact.i64(ptr addrspace(200) {{.*}}%s.0,
    // elide-NEXT: [[T:%.*]] = {{.*}}@llvm.cheri.cap.tag.get(ptr addrspace(200) [[E]])
    // elide-NEXT: br i1 [[T]], label %[[FAST:.*]], label %[[SLOW:cheri.slowcheck]]
    // elide-NOT: icmp
    // elide: [[FAST]]:
    // elide-NOT: icmp
    // elide: getelementptr [8 x i8], ptr addrspace(200) {{.*}}, i64 %i
    // elide-NEXT: load i64
    // elide: [[SLOW]]:
    // elide: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}%s.0,
    // elide: icmp ult i64 %i, %s.1
    // elide: panic_bounds_check
    s[i]
}

// CHECK-LABEL: @slice_store(
#[no_mangle]
pub fn slice_store(s: &mut [u32], i: usize, v: u32) {
    // elide: @llvm.cheri.cap.tag.get
    // elide: getelementptr [4 x i8]
    // elide-NEXT: store i32 %v
    // off-NOT: llvm.cheri
    s[i] = v;
}

// A read-modify-write of one element is one site.
// CHECK-LABEL: @slice_rmw(
#[no_mangle]
pub fn slice_rmw(s: &mut [u32], i: usize) {
    // elide: @llvm.cheri.cap.tag.get
    // elide: getelementptr [4 x i8]
    // elide-NEXT: load i32
    // elide: store i32
    s[i] = s[i].wrapping_add(1);
}

// In a loop the narrowing and the tag test are loop-invariant and hoisted.
// CHECK-LABEL: @loop_sum(
#[no_mangle]
pub fn loop_sum(s: &[u64], n: usize) -> u64 {
    // M5r R3 (D23): the generic passes hoist the (loop-invariant) check to the preheader,
    // so the tag test branches to the slow check before the loop.
    // elide: @llvm.cheri.cap.bounds.set.exact.i64(
    // elide: @llvm.cheri.cap.tag.get(
    // elide: br i1 %{{.*}}, label %{{.*}}, label %cheri.slowcheck
    // elide-NOT: llvm.cheri.cap.bounds.set.exact
    // elide: cheri.slowcheck:
    let mut t = 0u64;
    for i in 0..n {
        t = t.wrapping_add(s[i]);
    }
    t
}

// Field of a struct behind a reference.
pub struct S {
    pub pad: u32,
    pub arr: [u16; 8],
}
// CHECK-LABEL: @field_array(
#[no_mangle]
pub fn field_array(s: &S, i: usize) -> u16 {
    // elide-NOT: panic_bounds_check
    // elide: @llvm.cheri.cap.bounds.set.exact.i64({{.*}}, i64 16)
    // elide-NOT: panic_bounds_check
    // elide: ret i16
    s.arr[i]
}
