//! Glue between Slint's software renderer and a board's display/touch drivers. Boards run their
//! own loop around `render_frame`/`dispatch_touch` instead of Slint's event loop.

use slint::platform::software_renderer::{LineBufferProvider, MinimalSoftwareWindow, Rgb565Pixel};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{LogicalPosition, PhysicalSize, PlatformError};
use std::rc::Rc;

/// A board's display: pushes RGB565 pixels into a rectangle.
pub trait DisplayDriver {
    fn draw_bitmap(&self, x0: u16, y0: u16, x1: u16, y1: u16, pixels: &[u16]) -> anyhow::Result<()>;
}

/// A touch point in the board's logical (post-rotation) coordinates.
pub struct TouchEvent {
    pub x: i32,
    pub y: i32,
}

/// A board's touch controller.
pub trait TouchDriver {
    fn poll(&mut self) -> Option<TouchEvent>;
}

pub struct EspPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl EspPlatform {
    /// Registers the Slint platform (once per process) and returns its window.
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

    // Has no default implementation without slint's "std" feature.
    fn duration_since_start(&self) -> core::time::Duration {
        let us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
        core::time::Duration::from_micros(us.max(0) as u64)
    }
}

/// Renders a pending frame line by line straight to `display`.
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

/// Feeds a touch reading into Slint. The release reuses the last position because `clicked` only
/// fires when press and release both land inside the area.
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
