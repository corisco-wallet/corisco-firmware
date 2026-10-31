# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.2...v0.1.3) - 2026-10-31

### Added

- *(firmware)* rename confirmation setting to blind signing, default on ([#22](https://github.com/corisco-wallet/corisco-firmware/pull/22))

### Fixed

- *(firmware)* stop logging touch positions and pairing passkey to serial ([#19](https://github.com/corisco-wallet/corisco-firmware/pull/19))

### Other

- release ([#17](https://github.com/corisco-wallet/corisco-firmware/pull/17))
- apply minimal-comment policy, add rustfmt config, fix warnings ([#20](https://github.com/corisco-wallet/corisco-firmware/pull/20))
- describe default-deny confirmation and simplify architecture ([#18](https://github.com/corisco-wallet/corisco-firmware/pull/18))

## [0.1.2](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.1...v0.1.2) - 2026-10-31

### Added

- *(firmware)* rename confirmation setting to blind signing, default on ([#22](https://github.com/corisco-wallet/corisco-firmware/pull/22))

### Fixed

- *(firmware)* stop logging touch positions and pairing passkey to serial ([#19](https://github.com/corisco-wallet/corisco-firmware/pull/19))

### Other

- release ([#17](https://github.com/corisco-wallet/corisco-firmware/pull/17))
- apply minimal-comment policy, add rustfmt config, fix warnings ([#20](https://github.com/corisco-wallet/corisco-firmware/pull/20))

## [0.1.2](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.1...v0.1.2) - 2026-10-31

### Other

- release ([#17](https://github.com/corisco-wallet/corisco-firmware/pull/17))
- apply minimal-comment policy, add rustfmt config, fix warnings ([#20](https://github.com/corisco-wallet/corisco-firmware/pull/20))
- describe default-deny confirmation and simplify architecture ([#18](https://github.com/corisco-wallet/corisco-firmware/pull/18))

## [0.1.3](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.2...v0.1.3) - 2026-10-31

### Added

- *(firmware)* rename confirmation setting to blind signing, default on ([#22](https://github.com/corisco-wallet/corisco-firmware/pull/22))

### Fixed

- *(firmware)* stop logging touch positions and pairing passkey to serial ([#19](https://github.com/corisco-wallet/corisco-firmware/pull/19))

### Other

- apply minimal-comment policy, add rustfmt config, fix warnings ([#20](https://github.com/corisco-wallet/corisco-firmware/pull/20))
- describe default-deny confirmation and simplify architecture ([#18](https://github.com/corisco-wallet/corisco-firmware/pull/18))

## [0.1.2](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.1...v0.1.2) - 2026-10-31

### Added

- *(firmware)* rename confirmation setting to blind signing, default on ([#22](https://github.com/corisco-wallet/corisco-firmware/pull/22))

### Fixed

- *(firmware)* stop logging touch positions and pairing passkey to serial ([#19](https://github.com/corisco-wallet/corisco-firmware/pull/19))

### Other

- apply minimal-comment policy, add rustfmt config, fix warnings ([#20](https://github.com/corisco-wallet/corisco-firmware/pull/20))

## [0.1.2](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.1...v0.1.2) - 2026-10-31

### Other

- apply minimal-comment policy, add rustfmt config, fix warnings ([#20](https://github.com/corisco-wallet/corisco-firmware/pull/20))
- describe default-deny confirmation and simplify architecture ([#18](https://github.com/corisco-wallet/corisco-firmware/pull/18))

## [0.1.2](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.1...v0.1.2) - 2026-10-13

### Fixed

- *(firmware)* default-deny signing and restrict key tweaks ([#10](https://github.com/corisco-wallet/corisco-firmware/pull/10))

### Other

- simplify readme for first-time users ([#16](https://github.com/corisco-wallet/corisco-firmware/pull/16))

## [0.1.1](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.0...v0.1.1) - 2026-10-13

### Fixed

- *(firmware)* default-deny signing and restrict key tweaks ([#10](https://github.com/corisco-wallet/corisco-firmware/pull/10))

## [0.1.1](https://github.com/corisco-wallet/corisco-firmware/compare/v0.1.0...v0.1.1) - 2026-10-13

### Fixed

- *(firmware)* default-deny signing and restrict key tweaks ([#10](https://github.com/corisco-wallet/corisco-firmware/pull/10))

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
