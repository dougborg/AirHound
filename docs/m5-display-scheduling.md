# M5 periodic display scheduling

The periodic status renderer yields between ten completed synchronous steps:
header (including its short indicator), five body rows, divider, and three footer
rows. Both pages retain the original pixels, formatting and 500 ms timer between
frames. One small owned snapshot fixes page selection and values for the frame;
all global/critical-section reads finish before rendering. Each step creates and
drops its own 40-byte formatting buffer. SPI transactions finish synchronously
before the next yield. Firmware adds no framebuffer, DMA or heap allocation.

Startup remains synchronous: display reset, splash, two-second wait, splash
cleanup and complete first status frame all precede DISPLAY_READY. XIAO does not
compile this M5 renderer. The UART's 20 ms line deadline is unchanged.

The previous full-frame draw could monopolize one shared executor poll. The
nominal RGB565 pixel payload is about 44,400 bytes per frame (8.88 ms at 40 MHz),
but this excludes address commands, glyph iteration, SPI/CPU overhead and
interrupts. It is not a measured upper bound. Row-level yields create scheduling
opportunities; they do not prove a maximum UART poll gap, zero output loss, or
the cause of the observed serial stall. A UART deadline ends at FIFO acceptance,
not necessarily wire flush, so adding full serial wire time to display wire time
is not a rigorous worst-case bound.

Host tests exercise the real embedded-graphics DrawTarget path against a frozen
346 synchronous layout reference for both pages and maximum-width values. They
check one row/divider per poll, actual cooperative UART poll opportunities, and
cancellation followed by a complete different page. Removing yields reproduces
the scheduling-test failure. Synthetic clocks make no hardware latency claim.

Before hardware acceptance, the exact combined firmware must pass both target
builds and existing stack/static-memory gates. Measure maximum render-step time
and UART poll gaps under both pages, button changes, maximum-width values and
long output records. If a row is still too slow, smaller glyph batches are the
next bounded change; do not silently relax the UART deadline or turn recovered
timeouts into lossless acceptance.
