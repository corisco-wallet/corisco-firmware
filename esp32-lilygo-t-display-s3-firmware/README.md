# esp32-lilygo-t-display-s3-firmware

Rust firmware for the hardware signer on the LilyGo T-Display-S3: onboarding
(mnemonic generation/recovery), PIN-encrypted seed storage, a BLE GATT
service that answers signing requests from the phone app, and an on-device
UI (Slint) for the whole flow, including the physical confirm/decline
screen shown before any fund-moving signature is produced.

This crate holds everything specific to this one board (display/touch
drivers, pin assignments, the `.slint` UI layout, `main.rs`'s boot
sequence). The BLE protocol, seed storage, RNG, and Slint platform glue are
shared, board-agnostic logic living in
[`../firmware-core`](../firmware-core) -- see that crate's README for the
split and the `DisplayDriver`/`TouchDriver` traits a future board crate
would need to implement.

Targets the [LilyGo T-Display-S3](https://github.com/Xinyuan-LilyGO/T-Display-S3)
(ESP32-S3, ST7789 display, CST816S touch) -- see `src/display.rs` and
`src/touch.rs` for the exact pin mapping.

## Prerequisites

- The Xtensa Rust toolchain, via [`espup`](https://github.com/esp-rs/espup):

  ```bash
  cargo install espup
  espup install
  . $HOME/export-esp.sh   # source this in every new shell before building
  ```

- [`espflash`](https://github.com/esp-rs/espflash) for flashing:

  ```bash
  cargo install espflash
  ```

- A real device connected over USB. The panel is small (170x320,
  landscape) -- this firmware has not been ported to any other display.

## Building

This crate is a member of the Cargo workspace rooted at the repo root
(`corisco-wallet/Cargo.toml`), alongside `firmware-core`. Build from either
directory -- Cargo finds the workspace root automatically:

```bash
cargo build --release
```

Always use `--release` for anything involving real touch/UI interaction:
the workspace has no `[profile.dev]` override, so a debug build compiles
Slint's software renderer at `opt-level = 0`, which is noticeably too
slow to use.

`signer-core` (the signing logic) comes from
[crypto-core](https://github.com/corisco-wallet/crypto-core) -- see the
workspace root `Cargo.toml`'s `[workspace.dependencies]` table for how
that path dependency is currently resolved.

## Flashing

```bash
cargo run --release
```

(`.cargo/config.toml`'s `runner` is set to `espflash flash --monitor`, so
`cargo run` builds, flashes, and opens a serial monitor in one step.)

## First boot

A freshly flashed device starts at the Welcome screen: create a new
12/24-word wallet or recover an existing one, then set a PIN. The seed is
encrypted at rest (AES-256-GCM, PBKDF2-derived key) and only ever
decrypted into RAM after a correct PIN unlock. After `MAX_PIN_ATTEMPTS`
wrong attempts the encrypted seed is wiped (see `src/storage.rs`).

Once unlocked, the device advertises over BLE and is discoverable to the
mobile app (`corisco-android-app/`) for pairing.

## Testing

This crate has no host-testable logic of its own -- the actual signing
math lives in [crypto-core](https://github.com/corisco-wallet/crypto-core)
and is tested there. Verifying a change here means a real build + flash +
manual walkthrough of whatever screen/flow it touches.
