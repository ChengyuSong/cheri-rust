// M7.2h [LLM-generated]: upstream crate for stores-into-marks-xcrate.rs. Non-generic
// functions whose bodies store a pointer argument into another argument's memory;
// their marks are kept in this crate's metadata.
//@ compile-flags: -Cdebuginfo=0 -Copt-level=1

#![crate_type = "lib"]
#![no_std]

#[repr(C)]
pub struct Hold {
    pub a: u64,
    pub p: *const u64,
}

#[no_mangle]
#[inline(never)]
pub unsafe fn put_raw_x(dst: *mut *const u64, p: *const u64) {
    unsafe { *dst = p }
}

#[no_mangle]
#[inline(never)]
pub unsafe fn put_field_x(h: *mut Hold, p: *const u64) {
    unsafe { (*h).p = p }
}

#[no_mangle]
#[inline(never)]
pub unsafe fn put_var_x(dst: *mut *const u64, i: usize, p: *const u64) {
    unsafe { *dst.add(i) = p }
}
