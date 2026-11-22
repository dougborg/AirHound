/* Diagnostic-only adaptation of flipperzero-rt 0.16.0, commit
 * 08339c46b847610cc3a8a8a17651e7000e773ca9. MIT; see diagnostic-linker.LICENSE.
 * Original layout retained; only the .debug* discard is removed so a matching
 * debug ELF can retain line information. Standard demo linker is unchanged. */
ENTRY(_start);

SECTIONS
{
    .text 0x00000000 : ALIGN(4)
    {
        KEEP(*(.init));
        *(.text .text.*);
        KEEP(*(.fini));
    }

    .rodata :
    {
        *(.rodata .rodata.*);
    }

    .bss :
    {
        *(.bss .bss.*);
    }

    .data :
    {
        *(.data .data.*);
    }

	.ARM.attributes :
	{
		*(.ARM.attributes .ARM.attributes.*);
	}

    .fapmeta (INFO) :
    {
        KEEP(*(.fapmeta));
    }

    /DISCARD/ :
    {
        *(.text.strlen);

        /* LLVM bitcode should never be in the binary */
        *(.llvmbc .llvmcmd);

        /* Unwinding is not supported */
        *(.ARM.exidx .ARM.exidx.*);

        *(.comment .comment.*);
        *(.note .note.*);
    }
}
