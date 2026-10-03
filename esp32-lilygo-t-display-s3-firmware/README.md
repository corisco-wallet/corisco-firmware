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

`corisco-crypto-core` (the signing logic) comes from
[crypto-core](https://github.com/corisco-wallet/crypto-core) -- see the
workspace root `Cargo.toml`'s `[workspace.dependencies]` table for how
that path dependency is currently resolved.

## Flashing

```bash
cargo run --release
```

`.cargo/config.toml`'s `runner` is set to `espflash flash --monitor`, so
this one command builds, flashes, and opens a serial monitor on the
device's boot log, in that order, in a single `espflash` process. This
needs a real interactive terminal (the monitor reads your keystrokes for
`Ctrl+R`/`Ctrl+C`) -- it won't work piped through something non-interactive
(SSH without a TTY, a CI job, an agent's sandboxed shell).

If you're in a non-interactive context, or you just don't want the monitor
to block: build and flash as two separate steps instead, which skips the
monitor entirely:

```bash
cargo build --release
espflash flash --port /dev/ttyACM0 \
  target/xtensa-esp32s3-espidf/release/esp32-lilygo-t-display-s3-firmware
```

(`espflash board-info` will print the right `--port` if you're not sure
which device node it is; omit `--port` entirely if only one serial device
is connected and espflash can pick it for you.) To look at the boot log
afterward, watch it as its own step, started *before* you reset the
device, not concurrently with another command touching the same port --
having two processes toggle the port's DTR/RTS lines against each other at
the same time is how you accidentally land the chip back in the ROM
bootloader ("waiting for download") instead of a normal boot:

```bash
espflash monitor --port /dev/ttyACM0    # start this first
espflash reset --port /dev/ttyACM0      # then, from another shell, reset
```

On Linux, if flashing fails with a permissions error on the port, you're
probably not in the group that owns it (commonly `dialout` or `uucp`):
`groups` to check, `sudo usermod -aG dialout $USER` (then log out/in) to
fix it.

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
