OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

PHDRS
{
    limine_requests PT_LOAD FLAGS(4); /* PF_R */
    text            PT_LOAD FLAGS(5); /* PF_R | PF_X */
    rodata          PT_LOAD FLAGS(4); /* PF_R */
    data            PT_LOAD FLAGS(6); /* PF_R | PF_W */
}

PROVIDE(__fipsos_user_main_entry = 0);

SECTIONS
{
    /* Higher-Half Kernel base address required by Limine protocol */
    . = 0xFFFFFFFF80000000;

    .limine_requests : ALIGN(CONSTANT(MAXPAGESIZE)) {
        KEEP(*(.limine_requests_start_marker))
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end_marker))
        KEEP(*(.limine_requests_end))
    } :limine_requests

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .text : ALIGN(CONSTANT(MAXPAGESIZE)) {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .rodata : ALIGN(CONSTANT(MAXPAGESIZE)) {
        . = ALIGN(8);
        __fipsos_kernel_main_entry = .;
        KEEP(*(.rodata.*__FIPSOS_KERNEL_MAIN_ENTRY* .data.rel.ro.*__FIPSOS_KERNEL_MAIN_ENTRY* .data.*__FIPSOS_KERNEL_MAIN_ENTRY*))
        __fipsos_kernel_main_entry_end = .;
        *(.rodata .rodata.*)
        *(.data.rel.ro .data.rel.ro.*)
    } :rodata

    ASSERT(__fipsos_kernel_main_entry_end - __fipsos_kernel_main_entry == 8,
           "ERROR: Missing ostd::kernel_main! entry point in kernel_main")

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .data : ALIGN(CONSTANT(MAXPAGESIZE)) {
        *(.got .got.*)
        *(.data .data.*)
    } :data

    .bss : ALIGN(CONSTANT(MAXPAGESIZE)) {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.comment*)
    }
}
