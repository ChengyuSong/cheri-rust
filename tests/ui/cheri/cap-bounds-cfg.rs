// `cfg(cheri_cap_bounds_checks)` follows `narrow` and `elide` only.
//@ revisions: off narrow elide drop
//@ check-pass
//@ only-riscv64cheri-unknown-none-purecap
//@ [off] compile-flags: -Zcheri-cap-bounds-checks=off
//@ [narrow] compile-flags: -Zcheri-cap-bounds-checks=narrow
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
//@ [drop] compile-flags: -Zcheri-drop-bounds-checks
#![crate_type = "lib"]
#![no_std]
#![feature(cfg_cheri_cap_bounds_checks)]

#[cfg(all(off, cheri_cap_bounds_checks))]
compile_error!("off must not set cfg(cheri_cap_bounds_checks)");

#[cfg(all(narrow, not(cheri_cap_bounds_checks)))]
compile_error!("narrow must set cfg(cheri_cap_bounds_checks)");

#[cfg(all(elide, not(cheri_cap_bounds_checks)))]
compile_error!("elide must set cfg(cheri_cap_bounds_checks)");

#[cfg(all(drop, cheri_cap_bounds_checks))]
compile_error!("cheri-drop-bounds-checks must not set cfg(cheri_cap_bounds_checks)");
