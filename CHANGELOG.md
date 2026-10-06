# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.0...v0.1.1) - 2026-10-06

### Added

- Corisco-<id> BLE name, jade-style UI and logo ([#3](https://github.com/corisco-wallet/corisco-firmware/pull/3))

### Other

- update references after renaming the firmware repo to corisco-firmware
- add an app-only firmware image that preserves NVS and document flashing releases

## [0.1.0](https://github.com/corisco-wallet/corisco-firmware/releases/tag/v0.1.0) - 2026-10-06

### Added

- Corisco-<id> BLE name, jade-style UI and logo ([#3](https://github.com/corisco-wallet/corisco-firmware/pull/3))

### Other

- update references after renaming the firmware repo to corisco-firmware
- add an app-only firmware image that preserves NVS and document flashing releases
- move corisco-android-app to its own repo
- rename signer-core to corisco-crypto-core and pin it to the v0.1.0 git tag
- readme improvements
- Split firmware into firmware-core and esp32-lilygo-t-display-s3-firmware crates
