//! ST7789V bring-up over the LilyGo T-Display-S3(-Touch)'s 8-bit parallel
//! (Intel 8080 / "i80") bus.
//!
//! Uses ESP-IDF's `esp_lcd` component directly via `esp_idf_svc::sys` FFI
//! bindings, not a Rust SPI-display crate (`mipidsi` et al. don't support
//! the parallel i80 bus this board actually uses). Struct field
//! names/shapes below were read directly out of
//! this project's own generated bindgen output
//! (`target/xtensa-esp32s3-espidf/*/build/esp-idf-sys-*/out/bindings.rs`
//! for ESP_IDF_VERSION v5.5.3, pinned in `.cargo/config.toml`), not
//! guessed -- that's also why several config structs are built as full
//! field-by-field literals instead of `..Default::default()`: bindgen did
//! not derive `Default` for `esp_lcd_i80_bus_config_t` or
//! `esp_lcd_panel_io_i80_config_t` (they contain a raw union), so leaving
//! any field out wouldn't compile.
//!
//! Pin assignments and the "170x320 panel mounted on a 240-wide
//! controller" x-gap quirk are LilyGo's own published values (T-Display-S3
//! `pin_config.h` / factory example -- confirmed via the LilyGo repo's own
//! pin mapping docs, same wiring for the Touch variant since it's the same
//! display module with an add-on I2C touch panel).

use esp_idf_hal::task::notification::Notification;
use esp_idf_svc::sys::*;
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::sync::Arc;

// Landscape -- rotated 90 degrees from the panel's native portrait
// orientation via `esp_lcd_panel_swap_xy`/`esp_lcd_panel_mirror` below, so
// there's more horizontal room for larger text. `WIDTH`/`HEIGHT` here are
// the *logical*, post-rotation values everything else in this codebase
// (Slint's window size, `X_GAP`/`Y_GAP`, the DMA scratch buffer shape) is
// sized against.
pub const WIDTH: u16 = 320;
pub const HEIGHT: u16 = 170;

// The DMA scratch buffer covers this many rows at a time, not the whole
// screen -- a full-frame buffer failed to reliably allocate from
// DMA-capable memory this early in boot (real hardware: `memory
// allocation of 108800 bytes failed` -> abort -> reboot, when this was
// still a 170x320 portrait full frame). Espressif's own i80 LCD example
// uses the same row-banded approach for the same reason. In the current
// 320-wide landscape orientation, 20 rows costs 320*20*2 = 12,800 bytes,
// keeping the same order-of-magnitude scratch size as the original
// portrait value (170*40*2 = 13,600 bytes) despite the wider rows.
const ROWS_PER_CHUNK: u16 = 20;

const PIN_POWER_ON: gpio_num_t = 15;
const PIN_LCD_BL: gpio_num_t = 38;
const PIN_LCD_RES: gpio_num_t = 5;
const PIN_LCD_CS: gpio_num_t = 6;
const PIN_LCD_DC: gpio_num_t = 7;
const PIN_LCD_WR: gpio_num_t = 8;
// `esp_lcd_i80_bus_config_t` has no field for this at all -- the i80 bus
// driver only manages DC/WR/data/CS, not RD. Left unconfigured, this pin
// floats, which is a classic source of exactly the kind of persistent,
// clock-speed-invariant random noise seen on real hardware here: with RD
// undriven, the panel can't reliably tell write cycles from read cycles.
// Driven HIGH (inactive, matching WR/CS's active-low convention) once at
// init and never toggled again, since this driver never reads from the
// panel.
const PIN_LCD_RD: gpio_num_t = 9;
const PIN_LCD_D: [gpio_num_t; 8] = [39, 40, 41, 42, 45, 46, 47, 48];

// The physical 170x320 panel is mounted on a controller addressable as
// 240x320 -- the well-known T-Display-S3 quirk, hence the 35px gap
// (LilyGo's own factory example uses the same value in portrait, where it
// applies to X). `esp_lcd_panel_set_gap`'s x/y arguments are in the
// *post*-`swap_xy`/`mirror` logical coordinate space (applied below, in
// `new()`), so with the panel now rotated to landscape the 240-wide
// native axis is this orientation's logical Y axis -- best guess is the
// gap moves from X to Y accordingly. **Unconfirmed until the first real
// flash** (same as this project's other display quirks -- see the module
// doc comment): if the image comes up offset/cut off, try swapping which
// of these two is 35 and which is 0 before suspecting anything else.
const X_GAP: i32 = 0;
const Y_GAP: i32 = 35;

pub struct Display {
    panel: esp_lcd_panel_handle_t,
    io: esp_lcd_panel_io_handle_t,
    bus: esp_lcd_i80_bus_handle_t,
    // Signaled by `on_color_trans_done_cb` (fired from an ISR once the DMA
    // engine has actually finished reading a `draw_bitmap` buffer) --
    // `draw_bitmap` blocks on this before returning. `esp_lcd_panel_draw_bitmap`
    // on the i80 bus is asynchronous (queued via `trans_queue_depth`); without
    // this, the caller's pixel buffer could be freed/reused while the DMA
    // engine is still reading it -- a real use-after-free, fixed here on
    // principle even though it turned out not to be the visible cause of
    // the noise seen on real hardware (see `PIN_LCD_RD`'s doc comment for
    // what actually was).
    trans_done: Notification,
    // DMA-safe scratch buffer, allocated via `esp_lcd_i80_alloc_draw_buffer`
    // (NOT a plain Rust `Vec`/`malloc`) -- the i80 DMA engine needs memory
    // with the right burst/cache-line alignment, and plain heap memory
    // isn't guaranteed to have it (nor, on a PSRAM-equipped board like this
    // one, to be DMA-cache-coherent if it lands in PSRAM). Also fixed on
    // principle, not confirmed to be what actually caused the noise (see
    // `PIN_LCD_RD`) -- `draw_bitmap` copies into this buffer before
    // triggering a transfer, rather than handing the DMA engine a pointer
    // into arbitrary caller memory.
    dma_buf: *mut u16,
    dma_buf_len: usize,
}

/// Registered as `on_color_trans_done`. Runs in ISR context -- only
/// ISR-safe operations (the task notification) allowed here.
unsafe extern "C" fn on_color_trans_done_cb(
    _panel_io: esp_lcd_panel_io_handle_t,
    _edata: *mut esp_lcd_panel_io_event_data_t,
    user_ctx: *mut c_void,
) -> bool {
    let notifier = &*(user_ctx as *const esp_idf_hal::task::notification::Notifier);
    let (_, higher_prio_task_woken) = notifier.notify(NonZeroU32::new(1).unwrap());
    higher_prio_task_woken
}

// Deliberately NOT `Send`: `trans_done` (a `Notification`) captures the
// FreeRTOS task handle that constructed it, and `draw_bitmap` must be
// called from that same task -- see `Display::new`'s doc comment.

impl Display {
    pub fn new() -> anyhow::Result<Self> {
        // Must be created on the thread that will later call `draw_bitmap`
        // -- it captures the calling FreeRTOS task handle to notify.
        let trans_done = Notification::new();
        let notifier_ptr = Arc::as_ptr(&trans_done.notifier()) as *mut c_void;

        unsafe {
            raw_gpio_output(PIN_POWER_ON)?;
            gpio_set_level(PIN_POWER_ON, 1);
            raw_gpio_output(PIN_LCD_BL)?;
            gpio_set_level(PIN_LCD_BL, 0); // backlight stays off until init succeeds
            raw_gpio_output(PIN_LCD_RD)?;
            gpio_set_level(PIN_LCD_RD, 1); // tie inactive-high before the bus ever toggles WR

            // Let the panel's own power rail settle before driving it.
            esp_idf_svc::hal::delay::FreeRtos::delay_ms(50);

            let mut data_gpio_nums = [-1i32; 16];
            data_gpio_nums[..8].copy_from_slice(&PIN_LCD_D);

            let bus_config = esp_lcd_i80_bus_config_t {
                dc_gpio_num: PIN_LCD_DC,
                wr_gpio_num: PIN_LCD_WR,
                clk_src: soc_periph_lcd_clk_src_t_LCD_CLK_SRC_DEFAULT,
                data_gpio_nums,
                bus_width: 8,
                max_transfer_bytes: WIDTH as usize * ROWS_PER_CHUNK as usize * 2,
                __bindgen_anon_1: esp_lcd_i80_bus_config_t__bindgen_ty_1 { dma_burst_size: 64 },
                sram_trans_align: 4,
            };
            let mut bus: esp_lcd_i80_bus_handle_t = std::ptr::null_mut();
            esp!(esp_lcd_new_i80_bus(&bus_config, &mut bus))?;

            let mut dc_levels = esp_lcd_panel_io_i80_config_t__bindgen_ty_1::default();
            dc_levels.set_dc_idle_level(0);
            dc_levels.set_dc_cmd_level(0);
            dc_levels.set_dc_dummy_level(0);
            dc_levels.set_dc_data_level(1);

            let flags = esp_lcd_panel_io_i80_config_t__bindgen_ty_2::default();

            let io_config = esp_lcd_panel_io_i80_config_t {
                cs_gpio_num: PIN_LCD_CS,
                // Clock speed was never actually the problem (2MHz and
                // 10MHz produced identical noise before the real cause --
                // an undriven RD pin, see PIN_LCD_RD's doc comment -- was
                // fixed). Back to a real working speed now that the image
                // is confirmed clean.
                pclk_hz: 10_000_000,
                trans_queue_depth: 10,
                on_color_trans_done: Some(on_color_trans_done_cb),
                user_ctx: notifier_ptr,
                lcd_cmd_bits: 8,
                lcd_param_bits: 8,
                dc_levels,
                flags,
            };
            let mut io: esp_lcd_panel_io_handle_t = std::ptr::null_mut();
            esp!(esp_lcd_new_panel_io_i80(bus, &io_config, &mut io))?;

            let panel_config = esp_lcd_panel_dev_config_t {
                reset_gpio_num: PIN_LCD_RES,
                __bindgen_anon_1: esp_lcd_panel_dev_config_t__bindgen_ty_1 {
                    rgb_ele_order: lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_RGB,
                },
                // Green (0x07E0) showed as red on real hardware -- exactly
                // what byte-swapped RGB565 looks like (0x07E0 read as
                // 0xE007 decodes to mostly-red). BIG was the reference
                // example's value for a different panel/wiring; this board
                // needs the other order.
                data_endian: lcd_rgb_data_endian_t_LCD_RGB_DATA_ENDIAN_LITTLE,
                bits_per_pixel: 16,
                flags: esp_lcd_panel_dev_config_t__bindgen_ty_2::default(),
                vendor_config: std::ptr::null_mut(),
            };
            let mut panel: esp_lcd_panel_handle_t = std::ptr::null_mut();
            esp!(esp_lcd_new_panel_st7789(io, &panel_config, &mut panel))?;

            esp!(esp_lcd_panel_reset(panel))?;
            esp!(esp_lcd_panel_init(panel))?;
            // Rotates the panel's native portrait addressing 90 degrees
            // into the landscape `WIDTH`x`HEIGHT` this module now uses
            // everywhere else. `mirror_y: true` is a first guess at which
            // of the two 90-degree directions lands right-side-up given
            // how this board is physically held/mounted (USB-C edge on
            // the left) -- **unconfirmed until the first real flash**,
            // same as `X_GAP`/`Y_GAP` above: if the image is upside down
            // or mirrored, flip this to `false` (or flip `mirror_x`
            // instead) before suspecting anything else.
            esp!(esp_lcd_panel_swap_xy(panel, true))?;
            esp!(esp_lcd_panel_mirror(panel, false, true))?;
            esp!(esp_lcd_panel_set_gap(panel, X_GAP, Y_GAP))?;
            // Common ST7789-module quirk -- most of these boards render
            // inverted without this. Flip if the first fill-screen
            // milestone shows the wrong colors.
            esp!(esp_lcd_panel_invert_color(panel, true))?;
            esp!(esp_lcd_panel_disp_on_off(panel, true))?;

            let dma_buf_len = WIDTH as usize * ROWS_PER_CHUNK as usize;
            // DIAGNOSTIC: force internal RAM specifically, not just
            // "DMA-capable" -- on this PSRAM-equipped board, MALLOC_CAP_DMA
            // alone can be satisfied from PSRAM, which has real CPU-cache-
            // vs-DMA-engine coherency pitfalls a plain memory copy doesn't
            // account for. 13.6KB is trivial against the ~270KB+ free
            // internal RAM seen at boot, so this is fine to keep even once
            // it's confirmed to be the fix, not just a diagnostic.
            let dma_buf =
                esp_lcd_i80_alloc_draw_buffer(io, dma_buf_len * 2, MALLOC_CAP_DMA | MALLOC_CAP_INTERNAL) as *mut u16;
            anyhow::ensure!(!dma_buf.is_null(), "esp_lcd_i80_alloc_draw_buffer failed (out of DMA-capable memory)");

            gpio_set_level(PIN_LCD_BL, 1);

            Ok(Self { panel, io, bus, trans_done, dma_buf, dma_buf_len })
        }
    }

    /// Pushes `pixels` (RGB565, row-major) into the rectangle
    /// `[x0, x1) x [y0, y1)`. `pixels.len()` must equal
    /// `(x1 - x0) * (y1 - y0)`.
    ///
    /// Blocks until the DMA engine has actually finished reading `pixels`
    /// (via `on_color_trans_done_cb`/`trans_done`) before returning -- the
    /// underlying `esp_lcd_panel_draw_bitmap` call is asynchronous, so
    /// returning any earlier would let the caller free/reuse `pixels`
    /// while the transfer is still in flight. See `Display::trans_done`'s
    /// doc comment for what that looked like on real hardware.
    pub fn draw_bitmap(&self, x0: u16, y0: u16, x1: u16, y1: u16, pixels: &[u16]) -> anyhow::Result<()> {
        let expected = (x1 - x0) as usize * (y1 - y0) as usize;
        anyhow::ensure!(pixels.len() == expected, "draw_bitmap: got {} pixels, expected {expected}", pixels.len());
        anyhow::ensure!(
            expected <= self.dma_buf_len,
            "draw_bitmap: region ({expected} px) exceeds the DMA scratch buffer ({} px)",
            self.dma_buf_len
        );
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), self.dma_buf, expected);
            esp!(esp_lcd_panel_draw_bitmap(
                self.panel,
                x0 as i32,
                y0 as i32,
                x1 as i32,
                y1 as i32,
                self.dma_buf as *const c_void,
            ))?;
        }
        self.trans_done.wait_any();
        Ok(())
    }

    /// Fills the screen a solid color -- useful as a standalone check that
    /// the panel is wired and initialized correctly, independent of
    /// anything else (Slint, touch, etc.). Tiles down the screen in
    /// `ROWS_PER_CHUNK` bands rather than one full-frame draw, matching
    /// `dma_buf`'s size.
    pub fn fill_screen(&self, color: u16) -> anyhow::Result<()> {
        let band = vec![color; WIDTH as usize * ROWS_PER_CHUNK as usize];
        let mut y = 0u16;
        while y < HEIGHT {
            let rows = ROWS_PER_CHUNK.min(HEIGHT - y);
            let slice = &band[..WIDTH as usize * rows as usize];
            self.draw_bitmap(0, y, WIDTH, y + rows, slice)?;
            y += rows;
        }
        Ok(())
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            heap_caps_free(self.dma_buf as *mut c_void);
            esp_lcd_panel_del(self.panel);
            esp_lcd_panel_io_del(self.io);
            esp_lcd_del_i80_bus(self.bus);
        }
    }
}

impl firmware_core::platform::DisplayDriver for Display {
    fn draw_bitmap(&self, x0: u16, y0: u16, x1: u16, y1: u16, pixels: &[u16]) -> anyhow::Result<()> {
        Display::draw_bitmap(self, x0, y0, x1, y1, pixels)
    }
}

unsafe fn raw_gpio_output(pin: gpio_num_t) -> anyhow::Result<()> {
    esp!(gpio_set_direction(pin, gpio_mode_t_GPIO_MODE_OUTPUT))?;
    Ok(())
}
