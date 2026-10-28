//@ revisions: narrow drop bad
//@ check-fail
//@ [narrow] compile-flags: -Zcheri-cap-bounds-checks=narrow
//@ [narrow] ignore-riscv64cheri-unknown-none-purecap
//@ [narrow] ignore-riscv32cheriot-unknown-cheriotrtos
//@ [drop] compile-flags: -Zcheri-drop-bounds-checks
//@ [drop] ignore-riscv64cheri-unknown-none-purecap
//@ [drop] ignore-riscv32cheriot-unknown-cheriotrtos
//@ [bad] compile-flags: -Zcheri-cap-bounds-checks=bogus
#![crate_type = "lib"]
#![no_std]

//[narrow]~? ERROR `-Z cheri-cap-bounds-checks` is only supported on CHERI purecap targets
//[drop]~? ERROR `-Z cheri-drop-bounds-checks` is unsound and only supported on CHERI purecap targets
//[bad]~? ERROR incorrect value `bogus` for unstable option `cheri-cap-bounds-checks`
