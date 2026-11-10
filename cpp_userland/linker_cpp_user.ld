OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

PHDRS
{
    text    PT_LOAD FLAGS(5); /* PF_R | PF_X (1 | 4) - Strict W^X Code */
    rodata  PT_LOAD FLAGS(4); /* PF_R        (4)     - Strict W^X Read-Only Data */
    data    PT_LOAD FLAGS(6); /* PF_R | PF_W (2 | 4) - Strict W^X Read-Write Data */
}

SECTIONS
{
    /* Unified Canonical Linux x86_64 User Base (0x00400000) isolated by dedicated Process CR3/PML4 */
    . = 0x0000000000400000;

    .text ALIGN(0x1000) : {
        KEEP(*(.text._start))
        *(.text .text.*)
    } :text

    .rodata ALIGN(0x1000) : {
        *(.rodata .rodata.*)
        *(.got .got.*)
    } :rodata

    .data ALIGN(0x1000) : {
        *(.data .data.*)
    } :data

    .bss ALIGN(0x1000) : {
        *(COMMON)
        *(.bss .bss.*)
    } :data

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.comment)
    }
}
