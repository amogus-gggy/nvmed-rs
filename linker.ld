ENTRY(_start)

SECTIONS
{
  /* Lumd and SYS_EXEC load this flat image at different addresses. */
  . = 0;
  .text : {
    KEEP(*(.text._start))
    *(.text .text.*)
  }
  .rodata : { *(.rodata .rodata.*) }
  .data : { *(.data .data.*) }
  .got : { *(.got .got.*) }
  .rela.dyn : {
    relocations_start = .;
    *(.rela.dyn)
    relocations_end = .;
  }
  .bss : { *(.bss .bss.*) *(COMMON) }
  /DISCARD/ : { *(.eh_frame*) *(.comment*) *(.note*) }
}

ASSERT(_start == 0, "entry must be the first byte of the flat image")
