ENTRY(_start)

SECTIONS
{
  . = 0x500000;
  .text : {
    KEEP(*(.text._start))
    *(.text .text.*)
  }
  .rodata : { *(.rodata .rodata.*) }
  .data : { *(.data .data.*) }
  .got : { *(.got .got.*) }
  .rela.dyn : { *(.rela.dyn) }
  .bss : { *(.bss .bss.*) *(COMMON) }
  /DISCARD/ : { *(.eh_frame*) *(.comment*) *(.note*) }
}

ASSERT(_start == 0x500000, "entry must be the first byte of the flat image")
ASSERT(SIZEOF(.rela.dyn) == 0, "flat image cannot contain runtime relocations")
