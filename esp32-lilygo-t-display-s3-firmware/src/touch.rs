//! CST816S touch controller over I2C.

use cst816s::CST816S;
use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::{Input, InputPin, Output, OutputPin, PinDriver, Pull};
use esp_idf_hal::i2c::{I2cConfig, I2cDriver, I2C0};
use esp_idf_hal::units::FromValueType;
use firmware_core::platform::TouchEvent;

// The touch panel has no rotation setting, so raw portrait readings are remapped into the
// landscape display space.
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
        // Conservative; the CST816S supports up to 400kHz.
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

    /// Polls (no interrupt) for a touch, in logical display coordinates.
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
