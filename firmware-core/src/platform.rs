//! Bridges Slint's software renderer to a board's own display/touch
//! drivers, via the `DisplayDriver`/`TouchDriver` traits below -- this
//! module has no board-specific code of its own (see README.md).
//!
//! No `std` feature on the `slint` crate here (see Cargo.toml's comment on
//! why: it unconditionally cascades into a `memmap2` dependency that
//! doesn't build on this target's minimal libc). Without `std`,
//! `Platform::duration_since_start` has no default implementation and
//! must be provided -- backed by `esp_timer_get_time()`, the same clock
//! source already used elsewhere in this codebase (`selftest.rs`'s
//! benchmark timing, `ble.rs`'s self-test).
//!
//! This crate also doesn't implement `Platform::run_event_loop` --
//! `slint::platform::set_platform` is called once, but nothing calls
//! `.run()`/`run_event_loop()`. Instead each board's main/UI thread drives
//! its own loop (render + poll touch + check the BLE confirmation channel),
//! calling `render_frame` each iteration, rather than handing control to
//! Slint.

use slint::platform::software_renderer::{LineBufferProvider, MinimalSoftwareWindow, Rgb565Pixel};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{LogicalPosition, PhysicalSize, PlatformError};
use std::rc::Rc;

/// What `render_frame` needs from a board's display driver: push one
/// horizontal strip of RGB565 pixels to the panel. Implemented by each
/// board's own `display::Display` (e.g.
/// `esp32-lilygo-t-display-s3-firmware`'s ST7789V driver).
pub trait DisplayDriver {
    fn draw_bitmap(&self, x0: u16, y0: u16, x1: u16, y1: u16, pixels: &[u16]) -> anyhow::Result<()>;
}

/// A single touch reading, in the board's own logical (post-rotation)
/// coordinate space -- whatever that board's `touch::Touch::poll` already
/// remaps raw panel coordinates into.
pub struct TouchEvent {
    pub x: i32,
    pub y: i32,
}

/// What `dispatch_touch` needs from a board's touch driver. Implemented by
/// each board's own `touch::Touch` (e.g.
/// `esp32-lilygo-t-display-s3-firmware`'s CST816S driver).
pub trait TouchDriver {
    fn poll(&mut self) -> Option<TouchEvent>;
}

pub struct EspPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl EspPlatform {
    /// Registers itself as the active Slint platform (`slint::platform::set_platform`
    /// panics if called more than once per process, matching Slint's own
    /// contract -- fine here, this only ever runs once at boot). Returns
    /// the window handle the caller needs for `render_frame`/`dispatch_touch`.
    /// `width`/`height` are the board's own panel resolution (its
    /// `display` module's own `WIDTH`/`HEIGHT` constants).
    pub fn init(width: u16, height: u16) -> Rc<MinimalSoftwareWindow> {
        let window = MinimalSoftwareWindow::new(slint::platform::software_renderer::RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(width as u32, height as u32));

        slint::platform::set_platform(Box::new(EspPlatform { window: window.clone() }))
            .expect("set_platform must only be called once");

        window
    }
}

impl Platform for EspPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> core::time::Duration {
        let us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
        core::time::Duration::from_micros(us.max(0) as u64)
    }
}

/// Renders one frame if Slint has a pending redraw, pushing each dirty
/// line straight to `display` -- see `LineBufferProvider`'s own doc
/// example in `i-slint-renderer-software`, which this mirrors closely.
///
/// Per-line `draw_bitmap` calls (rather than batching several lines into
/// one DMA transfer, which a `DisplayDriver` could support) -- simplest
/// correct thing for this phase's milestone; revisit if full-screen
/// redraws turn out too slow once there's real UI to judge that against.
pub fn render_frame<D: DisplayDriver>(window: &MinimalSoftwareWindow, display: &D) {
    window.draw_if_needed(|renderer| {
        struct Provider<'a, D> {
            display: &'a D,
        }
        impl<D: DisplayDriver> LineBufferProvider for &mut Provider<'_, D> {
            type TargetPixel = Rgb565Pixel;

            fn process_line(
                &mut self,
                line: usize,
                range: core::ops::Range<usize>,
                render_fn: impl FnOnce(&mut [Self::TargetPixel]),
            ) {
                let mut buf = vec![Rgb565Pixel(0); range.len()];
                render_fn(&mut buf);
                let pixels: Vec<u16> = buf.iter().map(|p| p.0).collect();
                if let Err(e) = self.display.draw_bitmap(
                    range.start as u16,
                    line as u16,
                    range.end as u16,
                    line as u16 + 1,
                    &pixels,
                ) {
                    log::error!("platform: draw_bitmap failed for line {line}: {e:?}");
                }
            }
        }
        renderer.render_by_line(&mut Provider { display });
    });
}

/// Feeds a board's touch reading into the Slint window as pointer events.
/// `last_position` tracks the most recent contact point (`None` means "not
/// currently down"), since a `TouchDriver`'s `poll` reports "no touch" as
/// `None` rather than an explicit lift event with a final coordinate --
/// Slint's `TouchArea.clicked` only fires when both the press AND the
/// release land inside the area's bounds, so the release event must reuse
/// the last real position, not some fixed placeholder (a `(0, 0)` release
/// previously landed outside every button, silently swallowing every tap).
pub fn dispatch_touch<T: TouchDriver>(window: &MinimalSoftwareWindow, touch: &mut T, last_position: &mut Option<LogicalPosition>) {
    match touch.poll() {
        Some(ev) => {
            let position = LogicalPosition::new(ev.x as f32, ev.y as f32);
            let event = if last_position.is_some() {
                WindowEvent::PointerMoved { position }
            } else {
                log::info!("platform: PointerPressed at {position:?}");
                WindowEvent::PointerPressed { position, button: PointerEventButton::Left }
            };
            window.dispatch_event(event);
            *last_position = Some(position);
        }
        None => {
            if let Some(position) = last_position.take() {
                log::info!("platform: PointerReleased at {position:?}");
                window.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
                window.dispatch_event(WindowEvent::PointerExited);
            }
        }
    }
}
