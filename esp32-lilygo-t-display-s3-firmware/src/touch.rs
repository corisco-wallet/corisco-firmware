//! CST816S touch controller bring-up over I2C.
//!
//! Pins per LilyGo's T-Display-S3 Touch pin mapping (same source already
//! used for the display pins in `display.rs`, confirmed against the
//! user's own board documentation): SCL=17, SDA=18, INT=16, RST=21.
//!
//! Uses the `cst816s` crate (embedded-hal 1.0), which `esp-idf-hal` 0.46's
//! `I2cDriver`/`PinDriver` implement directly -- no raw FFI needed here,
//! unlike `display.rs`'s parallel-bus code (there's no equivalent
//! high-level Rust crate for the i80 bus, but I2C is a solved problem).

use cst816s::CST816S;
use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::{Input, InputPin, Output, OutputPin, PinDriver, Pull};
use esp_idf_hal::i2c::{I2cConfig, I2cDriver, I2C0};
use esp_idf_hal::units::FromValueType;
use firmware_core::platform::TouchEvent;

// The CST816S reports touch coordinates in the panel's fixed physical/
// glass orientation -- unlike the display controller, it has no
// equivalent of `esp_lcd_panel_swap_xy`/`mirror` (see `display.rs`) to
// follow the panel's software rotation, so raw readings still come back
// in the original 170(x)x320(y) portrait frame regardless of how the
// display itself is now addressed. `remap` below converts a raw reading
// into the new 320x170 landscape logical space `display.rs`'s
// `WIDTH`/`HEIGHT` and every Slint screen now use.
//
// Confirmed correct on real hardware (2026-09) after one iteration: the
// first guess (`raw_y`, `169 - raw_x`) put every observed tap's logical x
// *outside* the visible keypad's column range; flipping which end
// `raw_y` maps to instead fixed it, and real PIN entry (which needs both
// the right row and column) confirmed the y component too.
const NATIVE_PANEL_HEIGHT: i32 = 320;

fn remap(raw_x: i32, raw_y: i32) -> (i32, i32) {
    (NATIVE_PANEL_HEIGHT - 1 - raw_y, raw_x)
}

type Cst816sHandle = CST816S<I2cDriver<'static>, PinDriver<'static, Input>, PinDriver<'static, Output>>;

pub struct Touch {
    inner: Cst816sHandle,
}

impl Touch {
    pub fn new(
        i2c0: I2C0<'static>,
        sda: impl InputPin + OutputPin + 'static,
        scl: impl InputPin + OutputPin + 'static,
        int_pin: impl InputPin + 'static,
        rst_pin: impl OutputPin + 'static,
    ) -> anyhow::Result<Self> {
        // 100kHz: conservative starting point, same reasoning as
        // display.rs's original pclk_hz choice -- CST816S supports up to
        // 400kHz per its datasheet, bump once real hardware confirms
        // reliable reads.
        let i2c_config = I2cConfig::new().baudrate(100.kHz().into());
        let i2c = I2cDriver::new(i2c0, sda, scl, &i2c_config)?;
        let int_pin = PinDriver::input(int_pin, Pull::Up)?;
        let rst_pin = PinDriver::output(rst_pin)?;

        let mut inner = CST816S::new(i2c, int_pin, rst_pin);
        inner
            .setup(&mut FreeRtos)
            .map_err(|e| anyhow::anyhow!("cst816s setup failed: {e:?}"))?;

        Ok(Self { inner })
    }

    /// Polls for a touch event. `false` for `check_int_pin` -- plain
    /// polling rather than interrupt-driven wake; simpler, and cheap
    /// enough at this poll rate.
    pub fn poll(&mut self) -> Option<TouchEvent> {
        self.inner.read_one_touch_event(false).map(|e| {
            let (x, y) = remap(e.x, e.y);
            TouchEvent { x, y }
        })
    }
}

impl firmware_core::platform::TouchDriver for Touch {
    fn poll(&mut self) -> Option<TouchEvent> {
        Touch::poll(self)
    }
}
