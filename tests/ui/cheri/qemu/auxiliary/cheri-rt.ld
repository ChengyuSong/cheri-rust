/* Linker script for tests/ui/cheri/qemu (QEMU-CHERI -M spike, RAM at 0x80000000). */
OUTPUT_ARCH(riscv)
ENTRY(_start)
SECTIONS
{
  . = 0x80000000;
  .text : { *(.text.init) *(.text .text.*) }
  .rodata : { *(.rodata .rodata.*) *(.srodata .srodata.*) }
  __cap_relocs : { *(__cap_relocs) }
  . = ALIGN(0x1000);
  .data : { *(.data .data.*) *(.sdata .sdata.*) }
  .captable : { *(.captable) }
  .got : { *(.got) *(.got.plt) }
  .bss : { *(.bss .bss.*) *(.sbss .sbss.*) *(COMMON) }
  . = ALIGN(16);
  __stack_bottom = .;
  . += 0x10000;
  __stack_top = .;
  /* HTIF tohost/fromhost on their own page: QEMU maps it as MMIO (no tag storage). */
  . = ALIGN(0x1000);
  .tohost : { *(.tohost) }
  . = ALIGN(0x1000);
  _end = .;
}
