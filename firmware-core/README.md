# firmware-core

Board-agnostic firmware logic, shared across every ESP32/ESP-IDF board this
project supports. A board crate (e.g.
[`esp32-lilygo-t-display-s3-firmware`](../esp32-lilygo-t-display-s3-firmware))
depends on this as a library and supplies the pieces that are genuinely
specific to its own hardware.

## What's in here

- **`ble.rs`** -- the BLE GATT signing protocol (phone <-> device), built on
  `esp32-nimble`. No display/touch/UI coupling: this is the same wire
  protocol regardless of which board is running it.
- **`storage.rs`** -- PIN-encrypted seed storage on ESP-IDF's NVS, plus the
  PIN-attempt/lockout counter.
- **`rng.rs`** -- hardware TRNG entropy (`esp_fill_random`), for anything
  that ends up protecting real key material.
- **`platform.rs`** -- the Slint-to-hardware glue: an `EspPlatform` plus
  `render_frame`/`dispatch_touch` helpers that drive Slint's software
  renderer against *any* board's display/touch, via the traits below.
- **`selftest.rs`** -- boot-time crypto self-tests (`frost_self_test`,
  `bench_sha512_single_block`) that exercise `signer-core` against real
  hardware. Pure validation logic, no board coupling.

## What's deliberately NOT in here

- Any actual display or touch *driver* (panel bring-up, pin assignments,
  I2C/SPI/parallel-bus wiring).
- Any `.slint` UI layout, screen flow, or generated `AppWindow` -- those are
  tied to a specific board's screen resolution/aspect ratio and onboarding
  flow.
- `main.rs`'s boot sequence, peripheral/pin wiring, or UI event callbacks.

Those all stay in each board crate, since a new board can have a completely
different screen, touch controller, and pin mapping.

## The contract a new board crate implements

`platform::EspPlatform`/`render_frame`/`dispatch_touch` are generic over two
small traits, so a board only needs to implement these against its own
display/touch drivers to plug in:

```rust
pub trait DisplayDriver {
    fn draw_bitmap(&self, x0: u16, y0: u16, x1: u16, y1: u16, pixels: &[u16]) -> anyhow::Result<()>;
}

pub trait TouchDriver {
    fn poll(&mut self) -> Option<TouchEvent>; // TouchEvent { x: i32, y: i32 }
}
```

See `esp32-lilygo-t-display-s3-firmware/src/display.rs` and `src/touch.rs`
for the reference implementation (ST7789V over a parallel i80 bus, and a
CST816S touch controller over I2C, respectively).

Note: `main.rs`'s UI state machine (`ui/mod.rs` in each board crate) is
*not* abstracted behind a trait yet -- it talks directly to the
slint-generated `AppWindow` type, which is concrete per board. Sharing that
too would need a generic "AppWindow-like" interface; not worth designing
until there's a second board to validate the abstraction against.
