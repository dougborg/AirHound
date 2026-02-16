/// Hardware abstraction for supported boards.
///
/// Each board module defines pin assignments and capabilities
/// selected at compile time via feature flags.

// Pin assignments and capability flags are defined for hardware reference
// even when not yet wired up in code — peripherals are passed by type.
#[allow(dead_code)]
#[cfg(feature = "board-xiao")]
mod hw {
    pub const LED_PIN: u8 = 9; // WS2812 addressable LED
    pub const GPS_RX_PIN: u8 = 6;
    pub const GPS_TX_PIN: u8 = 5;
    pub const HAS_PSRAM: bool = true;
    pub const HAS_GPS_HEADER: bool = true;
    pub const HAS_DISPLAY: bool = false;
    pub const HAS_BUZZER: bool = true;
    pub const BUZZER_PIN: u8 = 3;
    pub const BUZZER_FREQ_HZ: u32 = 2000;
    pub const BUZZER_BEEP_MS: u64 = 200;
    pub const BOARD_NAME: &str = "xiao_esp32s3";
}

#[allow(dead_code)]
#[cfg(feature = "board-m5stickc")]
mod hw {
    pub const LED_PIN: u8 = 10; // Built-in LED
    pub const HAS_PSRAM: bool = false;
    pub const HAS_GPS_HEADER: bool = false;
    pub const HAS_DISPLAY: bool = true;
    pub const HAS_BUZZER: bool = true;
    pub const DISPLAY_WIDTH: u16 = 135;
    pub const DISPLAY_HEIGHT: u16 = 240;
    pub const BUZZER_PIN: u8 = 2;
    pub const BOARD_NAME: &str = "m5stickc_plus2";

    /// GPIO4 must be held HIGH to keep the device powered on
    pub const POWER_HOLD_PIN: u8 = 4;

    // Display SPI pins (ST7789V2) — peripherals are passed by type
    pub const DISPLAY_MOSI: u8 = 15;
    pub const DISPLAY_CLK: u8 = 13;
    pub const DISPLAY_CS: u8 = 5;
    pub const DISPLAY_DC: u8 = 14;
    pub const DISPLAY_RST: u8 = 12;
    pub const DISPLAY_BL: u8 = 27;
    pub const DISPLAY_SPI_FREQ_MHZ: u32 = 40;

    // Buzzer config
    pub const BUZZER_FREQ_HZ: u32 = 2700;
    pub const BUZZER_BEEP_MS: u64 = 150;
}

#[cfg(not(any(feature = "board-xiao", feature = "board-m5stickc")))]
mod hw {
    pub const BOARD_NAME: &str = "unknown";
}

pub use hw::*;
