ENTRY(_start)

/* Raspberry Pi 3 (BCM2837) AArch64 default kernel load address.
 * The GPU firmware (and QEMU's raspi3b -kernel loader) place a
 * kernel8.img-style AArch64 image here before releasing the ARM cores. */
BASE_ADDRESS = 0x80000;

SECTIONS
{
    . = BASE_ADDRESS;
    __start = .;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(16) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(16) {
        *(.data .data.*)
    }

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    . = ALIGN(16);
    __stack_bottom = .;
    . += 0x10000; /* 64 KiB boot stack, per core-0 only at this stage */
    __stack_top = .;

    __end = .;

    /DISCARD/ : {
        *(.comment)
        *(.note .note.*)
        *(.eh_frame)
    }
}
