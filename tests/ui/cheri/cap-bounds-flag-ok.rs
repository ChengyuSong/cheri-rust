//@ check-pass
//@ revisions: off dropno
//@ [off] compile-flags: -Zcheri-cap-bounds-checks=off
//@ [dropno] compile-flags: -Zcheri-drop-bounds-checks=no
#![crate_type = "lib"]
#![no_std]
