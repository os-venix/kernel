ENTRY(_start)
OUTPUT_FORMAT(elf64-x86-64)
KERNEL_OFFSET = 0xFFFFFFFF80000000;

PHDRS {
    text PT_LOAD;
    rodata PT_LOAD;
    data PT_LOAD;
}

SECTIONS {
    . = KERNEL_OFFSET;
    . += SIZEOF_HEADERS;

    .bootloader-config ALIGN(4K) : {
        *(.bootloader-config*)
    }
    .text : {
        __text_start = .;
        *(.text*)
        __usercopy_start = .;
        *(.usercopy-fns)
        __usercopy_end = .;
    }
    .rodata ALIGN(4K) : {
        __text_end = .;
        __rodata_start = .;
        *(.rodata*)
        __altcode_start = .;
        KEEP(*(.altcode*))
        __altcode_end = .;
        . = ALIGN(8);
        __altrelocs_start = .;
        KEEP(*(.altrelocs*))
        __altrelocs_end = .;
        __altfeatures_start = .;
        KEEP(*(.altfeatures*))
        __altfeatures_end = .;
    }
    .dynamic ALIGN(4K) : {
	*(.dynamic*);
    }
    .data ALIGN(4K) : {
        __rodata_end = .;
        __data_start = .;
        *(.data*)
        . = ALIGN(4K);
        __data_end = .;
        __bss_start = .;
        *(.bss*)

	KEEP(*(.requests_start_marker))
	KEEP(*(.requests))
	KEEP(*(.requests_end_marker))
    }
    .stack ALIGN(4K) : {
        *.(stack*)
    }
    __end = .;
    /DISCARD/ : {
        *(.comment*)
        *(.eh_frame*)
        *(.gcc_except_table*)
        *(.note*)
        *(.rel.eh_frame*)
    }
}
