//@ no-prefer-dynamic
//@ compile-flags: -Ccodegen-units=1
//! Bare-metal runtime for run-{pass,fail,crash} tests on riscv64cheri-unknown-none-purecap
//! under QEMU-CHERI (-M spike, HTIF), driven by src/etc/cheri-qemu-runner.sh.
//! LLM-generated research prototype (not for upstream).
//!
//! The test crate is `#![no_std] #![no_main]`, does `extern crate cheri_rt;`, links with
//! `-C link-arg=-T{{src-base}}/cheri/qemu/auxiliary/cheri-rt.ld`, and defines
//! `#[no_mangle] extern "C" fn cheri_test_main()`. Exit codes: see the runner script.
#![no_std]
#![crate_type = "rlib"]

use core::fmt::{self, Write};

core::arch::global_asm!(
    r#"
    .section .text.init, "ax"
    .globl _start
    .option push
    .option nocapmode
_start:
    # PCC -> capability mode
    cspecialr ct0, pcc
    lla       t1, 1f
    csetaddr  ct0, ct0, t1
    li        t2, 1
    csetflags ct0, ct0, t2
    jr.cap    ct0
    .option capmode
1:
    cspecialr cs0, ddc
    cspecialr cs1, pcc
    cllc      ct0, __cheri_rt_trap
    cspecialw mtcc, ct0
    cllc      ct0, __stack_bottom
    cllc      ct1, __stack_top
    sub       t2, t1, t0
    csetaddr  csp, cs0, t0
    csetbounds csp, csp, t2
    cincoffset csp, csp, t2
    # __cap_relocs: {{ u64 location, base, offset, size, flags }}
    cllc      ct3, __start___cap_relocs
    cllc      ct4, __stop___cap_relocs
2:  bgeu      t3, t4, 5f
    csetaddr  ca0, cs0, t3
    ld        t5, 0(ca0)
    ld        t6, 8(ca0)
    ld        a2, 16(ca0)
    ld        a3, 24(ca0)
    ld        a4, 32(ca0)
    cmove     ca1, cs0
    bgez      a4, 3f
    cmove     ca1, cs1
3:  csetaddr  ca1, ca1, t6
    beqz      a3, 4f
    csetbounds ca1, ca1, a3
4:  cincoffset ca1, ca1, a2
    csetaddr  ca0, cs0, t5
    sc        ca1, 0(ca0)
    addi      t3, t3, 40
    j         2b
5:
    ccall     cheri_test_main
    li        a0, 0
    j         __cheri_rt_exit

    .align 2
__cheri_rt_trap:                 # mcause 28 -> 128 + (mtval & 0x1f); else 192 + mcause
    csrr      a0, mcause
    csrr      a1, mtval
    li        t1, 28
    bne       a0, t1, 7f
    andi      a1, a1, 0x1f
    addi      a0, a1, 128
    j         __cheri_rt_exit
7:  andi      a0, a0, 0x3f
    addi      a0, a0, 192

    .globl __cheri_rt_exit
__cheri_rt_exit:                 # HTIF exit: tohost = (code << 1) | 1
    cspecialr cs0, ddc
    cllc      ct0, tohost
    csetaddr  ca5, cs0, t0
    slli      a0, a0, 1
    ori       a0, a0, 1
    sd        a0, 0(ca5)
6:  j         6b
    .option pop

    .section .tohost, "aw", @progbits
    .align 6
    .globl tohost
tohost: .dword 0
    .size tohost, 8
    .align 6
    .globl fromhost
fromhost: .dword 0
    .size fromhost, 8
    .text
"#
);

unsafe extern "C" {
    fn __cheri_rt_exit(code: u64) -> !;
    static mut tohost: u64;
    static mut fromhost: u64;
}

/// Terminate the test with `code` (0 = pass, 101 = panic, see runner).
pub fn exit(code: u8) -> ! {
    unsafe { __cheri_rt_exit(code as u64) }
}

/// HTIF console (device 1, cmd 1 = putchar); QEMU writes it to its stdout.
pub fn putc(c: u8) {
    unsafe {
        let th = &raw mut tohost;
        let fh = &raw mut fromhost;
        th.write_volatile((1u64 << 56) | (1u64 << 48) | c as u64);
        while fh.read_volatile() == 0 {}
        fh.write_volatile(0);
    }
}

pub struct Console;
impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            putc(b);
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! println {
    ($($t:tt)*) => {{
        let _ = core::fmt::Write::write_fmt(&mut $crate::Console, format_args!($($t)*));
        $crate::putc(b'\n');
    }};
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // Message on its own line so tests can match it in .run.stdout without the location.
    let mut c = Console;
    if let Some(l) = info.location() {
        let _ = write!(c, "panicked at {}:{}:{}:\n", l.file(), l.line(), l.column());
    }
    let _ = write!(c, "{}\n", info.message());
    exit(101)
}
