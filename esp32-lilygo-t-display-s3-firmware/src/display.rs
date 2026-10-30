//! ST7789V driver over the T-Display-S3's 8-bit parallel (i80) bus, using ESP-IDF's `esp_lcd`
//! directly since Rust display crates don't support i80. Config structs are full literals
//! because bindgen derives no `Default` for them.

use esp_idf_hal::task::notification::Notification;
use esp_idf_svc::sys::*;
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::sync::Arc;

// Logical landscape size, rotated from the panel's native portrait in `Display::new`.
pub const WIDTH: u16 = 320;
pub const HEIGHT: u16 = 170;

// The DMA buffer covers this many rows: a full-frame buffer fails to allocate this early in boot.
const ROWS_PER_CHUNK: u16 = 20;

const PIN_POWER_ON: gpio_num_t = 15;
const PIN_LCD_BL: gpio_num_t = 38;
const PIN_LCD_RES: gpio_num_t = 5;
const PIN_LCD_CS: gpio_num_t = 6;
const PIN_LCD_DC: gpio_num_t = 7;
const PIN_LCD_WR: gpio_num_t = 8;
// RD isn't managed by the i80 driver; left floating, the panel can't tell writes from reads and
// shows random noise, so it's tied inactive-high.
const PIN_LCD_RD: gpio_num_t = 9;
const PIN_LCD_D: [gpio_num_t; 8] = [39, 40, 41, 42, 45, 46, 47, 48];

// The 170px panel sits on a 240-wide controller; after `swap_xy` that offset is on Y.
const X_GAP: i32 = 0;
const Y_GAP: i32 = 35;

pub struct Display {
    panel: esp_lcd_panel_handle_t,
    io: esp_lcd_panel_io_handle_t,
    bus: esp_lcd_i80_bus_handle_t,
    // Signaled from the ISR when DMA finishes; `draw_bitmap` waits on it before reusing `dma_buf`.
    trans_done: Notification,
    // From `esp_lcd_i80_alloc_draw_buffer`: DMA needs aligned, cache-coherent memory.
    dma_buf: *mut u16,
    dma_buf_len: usize,
}

/// Runs in ISR context: only the task notification is safe here.
unsafe extern "C" fn on_color_trans_done_cb(
    _panel_io: esp_lcd_panel_io_handle_t,
    _edata: *mut esp_lcd_panel_io_event_data_t,
    user_ctx: *mut c_void,
) -> bool {
    let notifier = &*(user_ctx as *const esp_idf_hal::task::notification::Notifier);
    let (_, higher_prio_task_woken) = notifier.notify(NonZeroU32::new(1).unwrap());
    higher_prio_task_woken
}

impl Display {
    pub fn new() -> anyhow::Result<Self> {
        // Captures the calling FreeRTOS task: `draw_bitmap` must run on this same thread.
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
                // BIG showed green as red on this board (byte-swapped RGB565).
                data_endian: lcd_rgb_data_endian_t_LCD_RGB_DATA_ENDIAN_LITTLE,
                bits_per_pixel: 16,
                flags: esp_lcd_panel_dev_config_t__bindgen_ty_2::default(),
                vendor_config: std::ptr::null_mut(),
            };
            let mut panel: esp_lcd_panel_handle_t = std::ptr::null_mut();
            esp!(esp_lcd_new_panel_st7789(io, &panel_config, &mut panel))?;

            esp!(esp_lcd_panel_reset(panel))?;
            esp!(esp_lcd_panel_init(panel))?;
            // Landscape, right side up with the USB-C edge on the left.
            esp!(esp_lcd_panel_swap_xy(panel, true))?;
            esp!(esp_lcd_panel_mirror(panel, false, true))?;
            esp!(esp_lcd_panel_set_gap(panel, X_GAP, Y_GAP))?;
            // This ST7789 module renders inverted colors otherwise.
            esp!(esp_lcd_panel_invert_color(panel, true))?;
            esp!(esp_lcd_panel_disp_on_off(panel, true))?;

            let dma_buf_len = WIDTH as usize * ROWS_PER_CHUNK as usize;
            // Internal RAM only: MALLOC_CAP_DMA alone may land in PSRAM, which isn't DMA-coherent.
            let dma_buf =
                esp_lcd_i80_alloc_draw_buffer(io, dma_buf_len * 2, MALLOC_CAP_DMA | MALLOC_CAP_INTERNAL) as *mut u16;
            anyhow::ensure!(!dma_buf.is_null(), "esp_lcd_i80_alloc_draw_buffer failed (out of DMA-capable memory)");

            gpio_set_level(PIN_LCD_BL, 1);

            Ok(Self { panel, io, bus, trans_done, dma_buf, dma_buf_len })
        }
    }

    /// Draws RGB565 `pixels` into `[x0, x1) x [y0, y1)`, blocking until DMA has read them.
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
