//@ check-pass
//@ only-riscv64cheri-unknown-none-purecap
//@ revisions: narrow elide
//@ [narrow] compile-flags: -Zcheri-cap-bounds-checks=narrow
//@ [elide] compile-flags: -Zcheri-cap-bounds-checks=elide
#![crate_type = "lib"]
#![no_std]
