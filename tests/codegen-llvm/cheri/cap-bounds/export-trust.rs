// M3.0 export-trust fix: a function with a non-Rust ABI can be called from C, which may pass a
// capability wider than the referent (e.g. an arena-wide malloc capability). Its safe-pointer
// parameters are therefore NOT "mem.alloc_inbounds"; they are narrowed once at entry to the
// referent with a plain (non-access) llvm.cheri.cap.bounds.set, which CheriSkipNarrow never
// removes. Rust-ABI functions keep the M2 trust. `nofix` (-Zcheri-export-entry-bounds=no)
// reproduces the M2 behaviour.
//
//@ revisions: off skip noskip nofix
//@ compile-flags: -Copt-level=2
//@ [skip] compile-flags: -Zcheri-cap-bounds-checks=narrow
//@ [noskip] compile-flags: -Zcheri-cap-bounds-checks=narrow -Cllvm-args=-cheri-skip-narrow=false
//@ [nofix] compile-flags: -Zcheri-cap-bounds-checks=narrow -Zcheri-export-entry-bounds=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
#![allow(improper_ctypes_definitions)]

// A callback-style (address-taken, not exported) extern "C" fn is narrowed too.
// CHECK-LABEL: define internal {{.*}}@{{.*}}2cb(
// off: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}, i64 32)
// skip: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}, i64 32)
// nofix-NOT: llvm.cheri.cap.bounds.set
extern "C" fn cb(a: &[u64; 4]) -> u64 {
    a[1]
}
#[no_mangle]
pub fn take_cb() -> extern "C" fn(&[u64; 4]) -> u64 {
    cb
}

// Rust ABI: trusted (callers are Rust), no entry narrow; skip removes the access narrow.
// CHECK-LABEL: @rust_idx(
// CHECK-SAME: "mem.alloc_inbounds" "mem.range"="1:8" %s.0
// off-NOT: llvm.cheri.cap.bounds.set
// skip-NOT: llvm.cheri.cap.bounds.set
// nofix-NOT: llvm.cheri.cap.bounds.set
// noskip: llvm.cheri.cap.bounds.set{{.*}}!cheri.access_narrow
// CHECK: ret i64
#[no_mangle]
pub fn rust_idx(s: &[u64], i: usize) -> u64 {
    s[i]
}

// extern "C" &[T; N]: no trust; narrowed once at entry to 256 bytes in every mode.
// CHECK-LABEL: @c_ref(
// off-NOT: "mem.alloc_inbounds"
// skip-NOT: "mem.alloc_inbounds"
// noskip-NOT: "mem.alloc_inbounds"
// nofix-SAME: "mem.alloc_inbounds"
// off: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}%0, i64 256)
// skip: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}%0, i64 256)
// skip-NOT: !cheri.access_narrow
// noskip: @llvm.cheri.cap.bounds.set{{.*}}(ptr addrspace(200) {{.*}}%0, i64 256)
// nofix-NOT: llvm.cheri.cap.bounds.set
// CHECK: ret i64
#[no_mangle]
pub extern "C" fn c_ref(a: &[u64; 32], i: usize) -> u64 {
    a[i]
}

// extern "C" &[T] (passed indirectly): narrowed at entry to len * 8 bytes.
// CHECK-LABEL: @c_slice(
// off: [[LEN:%.*]] = shl {{.*}}i64 %{{.*}}, 3
// off: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}, i64 [[LEN]])
// skip: @llvm.cheri.cap.bounds.set.i64(
// skip-NOT: !cheri.access_narrow
// noskip: @llvm.cheri.cap.bounds.set.i64(
// nofix-NOT: llvm.cheri.cap.bounds.set
// CHECK: ret i64
#[no_mangle]
pub extern "C" fn c_slice(s: &[u64], i: usize) -> u64 {
    s[i]
}

// extern "C" &mut T: narrowed to size_of::<T>().
// CHECK-LABEL: @c_mut(
// off: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}%0, i64 4)
// skip: @llvm.cheri.cap.bounds.set.i64(ptr addrspace(200) {{.*}}%0, i64 4)
// nofix-NOT: llvm.cheri.cap.bounds.set
// CHECK: ret void
#[no_mangle]
pub extern "C" fn c_mut(x: &mut u32) {
    *x = 1
}

// Raw pointers carry no type contract: no trust, no entry narrow (M3.1+ contracts).
// CHECK-LABEL: @c_raw(
// CHECK-NOT: "mem.alloc_inbounds"
// CHECK-NOT: llvm.cheri.cap.bounds.set
// CHECK: ret i64
#[no_mangle]
pub extern "C" fn c_raw(p: *const u64) -> u64 {
    unsafe { *p }
}

// Zero-sized referent: nothing to narrow (a dangling pointer is valid).
// CHECK-LABEL: @c_zst(
// CHECK-NOT: llvm.cheri.cap.bounds.set
// CHECK: ret i8
#[no_mangle]
pub extern "C" fn c_zst(_z: &(), x: u8) -> u8 {
    x
}

