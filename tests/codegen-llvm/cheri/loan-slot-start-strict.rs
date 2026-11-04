// M5r R8 (D64) [LLM-generated]: after MIR inlining, `Location::file()`'s internal
// `&self.filename` loan start lost its `Ref` (folded by GVN), and the R5-R7 rule paired it
// with the next `Ref` of the block: the `&str` slot handed to `fmt::Arguments` got that
// short loan's `!mem.loan`, and the default loan mode cleared it before formatting (a tag
// fault in every panic message). A start now pairs only with the next assignment in its
// own source scope. `old` = the R7 rule.
//
//@ revisions: new old
//@ compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=narrow
//@[old] compile-flags: -Zcheri-slot-start-strict=no -Zcheri-slot-start-link=no
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]
use core::fmt::Write;
pub struct W;
impl Write for W {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        unsafe { sink(s.as_ptr()) };
        Ok(())
    }
}
unsafe extern "Rust" {
    fn sink(p: *const u8);
}

// CHECK-LABEL: @loc(
// new-NOT: !mem.loan
// old: !mem.loan
// CHECK: ret void
#[unsafe(no_mangle)]
pub fn loc(l: &core::panic::Location<'_>) {
    let _ = write!(W, "{}:{}", l.file(), l.line());
}
