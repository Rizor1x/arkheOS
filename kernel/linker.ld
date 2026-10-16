OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

PHDRS {
    requests PT_LOAD FLAGS(6);
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 0xffffffff80000000;
    __kernel_start = .;
    .requests : {
        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))
    } :requests

    . = ALIGN(0x1000);
    __text_start = .;
    .text : { *(.text .text.*) } :text
    . = ALIGN(0x1000);
    __rodata_start = .;
    .rodata : { *(.rodata .rodata.*) } :rodata
    . = ALIGN(0x1000);
    __data_start = .;
    .data : { *(.data .data.*) *(.got .got.*) } :data
    .bss : { *(.bss .bss.*) *(COMMON) } :data
    . = ALIGN(0x1000);
    __kernel_end = .;
    /DISCARD/ : { *(.eh_frame*) *(.note*) }
}
