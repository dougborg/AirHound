/// Hardware abstraction for supported boards.
///
/// Each board module defines pin assignments and capabilities
/// selected at compile time via feature flags.

#[cfg(feature = "xiao")]
mod hw {
    pub const LED_PIN: u8 = 9; // WS2812 addressable LED
    pub const GPS_RX_PIN: u8 = 6;
    pub const GPS_TX_PIN: u8 = 5;
    pub const HAS_PSRAM: bool = true;
    pub const HAS_GPS_HEADER: bool = true;
    pub const HAS_DISPLAY: bool = false;
    pub const HAS_BUZZER: bool = false;
    pub const BOARD_NAME: &str = "xiao_esp32s3";
}

#[cfg(feature = "m5stickc")]
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
}

#[cfg(not(any(feature = "xiao", feature = "m5stickc")))]
mod hw {
    pub const BOARD_NAME: &str = "unknown";
}

pub use hw::*;
