//@ check-pass
#![crate_type = "lib"]
#![no_std]
#![feature(core_intrinsics)]

const fn narrow(p: *const u8, n: usize) -> *const u8 {
    unsafe { core::intrinsics::cheri_bounds_narrow(p, n) }
}
const fn narrow_exact(p: *const u8, n: usize) -> *const u8 {
    unsafe { core::intrinsics::cheri_bounds_narrow_exact(p, n) }
}
const fn invalidate(p: *const u8) -> *const u8 {
    unsafe { core::intrinsics::cheri_invalidate(p) }
}
const fn narrow_slice(p: *const [u8], n: usize) -> *const [u8] {
    unsafe { core::intrinsics::cheri_bounds_narrow(p, n) }
}

const P: *const u8 = 0x1000 as *const u8;

const _: () = assert!(narrow(P, 4).addr() == P.addr());
const _: () = assert!(narrow_exact(P, 4).addr() == P.addr());
const _: () = assert!(invalidate(P).addr() == P.addr());

const S: *const [u8] = narrow_slice(core::ptr::slice_from_raw_parts(P, 8), 3);
const _: () = assert!(S.len() == 8);
const _: () = assert!(S.as_ptr().addr() == P.addr());
