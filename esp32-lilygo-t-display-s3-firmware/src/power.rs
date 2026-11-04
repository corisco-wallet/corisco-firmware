//! Long-press power button. The board has no power latch, so "off" is deep sleep with the LCD rail
//! cut; waking reboots the chip, which drops the keys from RAM and forces a fresh PIN unlock.

use crate::display::{Display, PIN_LCD_BL, PIN_POWER_ON};
use crate::AppWindow;
use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::{Input, InputPin, PinDriver, Pull};
use esp_idf_svc::sys::*;
use firmware_core::platform;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use std::time::{Duration, Instant};

// GPIO14 ("KEY"), not GPIO0: GPIO0 is a strapping pin and held at reset enters download mode.
const PIN_BUTTON: gpio_num_t = 14;
const HOLD: Duration = Duration::from_millis(1500);
const POLL_MS: u32 = 20;

pub struct PowerButton {
    pin: PinDriver<'static, Input>,
    pressed_since: Option<Instant>,
    fired: bool,
}

impl PowerButton {
    pub fn new(pin: impl InputPin + 'static) -> anyhow::Result<Self> {
        // After an ext0 wake the pad is still routed to the RTC mux; reclaim it for digital reads.
        unsafe { rtc_gpio_deinit(PIN_BUTTON) };
        let pin = PinDriver::input(pin, Pull::Up)?;
        Ok(Self { pin, pressed_since: None, fired: false })
    }

    fn is_pressed(&self) -> bool {
        self.pin.is_low()
    }

    /// Call once per loop iteration; true exactly once per hold that reaches `HOLD`.
    pub fn long_pressed(&mut self) -> bool {
        if !self.is_pressed() {
            self.pressed_since = None;
            self.fired = false;
            return false;
        }
        let since = *self.pressed_since.get_or_insert_with(Instant::now);
        if !self.fired && since.elapsed() >= HOLD {
            self.fired = true;
            return true;
        }
        false
    }

    fn wait_for_release(&self) {
        while self.is_pressed() {
            FreeRtos::delay_ms(POLL_MS);
        }
    }
}

/// Undoes the pin holds from the last power-off and, after a button wake, goes straight back to
/// sleep unless the button stays held, so a brief bump doesn't turn the device on.
pub fn on_boot(button: &PowerButton) {
    unsafe {
        gpio_hold_dis(PIN_POWER_ON);
        gpio_hold_dis(PIN_LCD_BL);
        gpio_deep_sleep_hold_dis();
    }
    let woke_by_button = unsafe { esp_sleep_get_wakeup_cause() } == esp_sleep_source_t_ESP_SLEEP_WAKEUP_EXT0;
    if !woke_by_button {
        return;
    }
    // Measured from app start; the bootloader has already eaten a few hundred ms of the hold.
    let boot = Instant::now();
    while boot.elapsed() < HOLD {
        if !button.is_pressed() {
            ::log::info!("power: button released before hold time, back to sleep");
            deep_sleep(button);
        }
        FreeRtos::delay_ms(POLL_MS);
    }
}

/// Shows "Powering off...", waits for the button to be released, then sleeps.
pub fn power_off(ui: &AppWindow, window: &MinimalSoftwareWindow, display: &Display, button: &PowerButton) -> ! {
    ui.set_working_label("Powering off...".into());
    ui.set_screen(6);
    platform::render_frame(window, display);
    button.wait_for_release();
    if let Err(e) = display.sleep() {
        ::log::error!("power: display sleep failed: {e:?}");
    }
    deep_sleep(button)
}

/// Enters deep sleep with the LCD rail held off; pressing the button wakes (reboots) the chip.
pub fn deep_sleep(button: &PowerButton) -> ! {
    // ext0 is level-triggered: sleeping while still held would wake immediately.
    button.wait_for_release();
    ::log::info!("power: entering deep sleep");
    unsafe {
        // Digital pads float in deep sleep unless held, which could leave the LCD rail powered.
        // Also reached before `Display::new` configured these as outputs.
        gpio_set_direction(PIN_LCD_BL, gpio_mode_t_GPIO_MODE_OUTPUT);
        gpio_set_direction(PIN_POWER_ON, gpio_mode_t_GPIO_MODE_OUTPUT);
        gpio_set_level(PIN_LCD_BL, 0);
        gpio_set_level(PIN_POWER_ON, 0);
        gpio_hold_en(PIN_LCD_BL);
        gpio_hold_en(PIN_POWER_ON);
        gpio_deep_sleep_hold_en();

        esp_sleep_enable_ext0_wakeup(PIN_BUTTON, 0);
        rtc_gpio_pullup_en(PIN_BUTTON);
        rtc_gpio_pulldown_dis(PIN_BUTTON);
        esp_deep_sleep_start();
    }
}
