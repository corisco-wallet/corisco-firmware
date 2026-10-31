# esp32-lilygo-t-display-s3-firmware

Firmware for the Corisco hardware signer on the
[LilyGo T-Display-S3](https://github.com/Xinyuan-LilyGO/T-Display-S3): wallet
setup, PIN-protected seed storage, Bluetooth signing for the phone app, and
the on-device screen where you confirm payments (when blind signing is off). Shared logic lives in
[`../firmware-core`](../firmware-core).

## Flash a release

Download the image from the [latest release](https://github.com/corisco-wallet/corisco-firmware/releases/latest) and install [`espflash`](https://github.com/esp-rs/espflash) (`cargo install espflash`).

New device:

```bash
espflash write-bin 0x0 esp32-lilygo-t-display-s3-firmware-vX.Y.Z.bin
espflash reset
```

On Linux, a permission error on the port means you need the `dialout` group:
`sudo usermod -aG dialout $USER`, then log out and back in.

## Updating

To update a device that already has a wallet, use the `-app.bin` image instead.
It keeps your seed, PIN and pairing:

```bash
espflash write-bin 0x10000 esp32-lilygo-t-display-s3-firmware-vX.Y.Z-app.bin
espflash reset
```

The full image erases the wallet from the device (you'd have to restore it from your recovery words and re-pair).

## First boot

The device starts at the Welcome screen: create a new wallet or recover one,
then set a PIN. After too many wrong PIN attempts the device wipes its
stored seed, so keep your recovery words safe. Once unlocked, it shows up
over Bluetooth as `Corisco-<id>` for the
[app](https://github.com/corisco-wallet/corisco-android-app) to pair with.

## Build from source

Install the Xtensa toolchain with [`espup`](https://github.com/esp-rs/espup) and `espflash`:

```bash
cargo install espup espflash
espup install
. $HOME/export-esp.sh   # in every new shell
```

Then, with the device plugged in:

```bash
cargo run --release     # builds, flashes and opens the serial monitor
```

Always use `--release`: debug builds are too slow to use the touch UI.
Without a terminal (or to skip the monitor), run `cargo build --release`
and then `espflash flash target/xtensa-esp32s3-espidf/release/esp32-lilygo-t-display-s3-firmware`.
