//! Board-agnostic firmware logic, shared across every ESP32/ESP-IDF board
//! this project supports: the BLE GATT signing protocol, PIN-encrypted NVS
//! seed storage, hardware RNG, the Slint platform/render glue, and the
//! boot-time crypto self-tests. See README.md for what's deliberately left
//! out (any display/touch driver, `.slint` UI layout, pin assignments, or
//! `main.rs` boot sequence -- those are each board crate's own job).

pub mod ble;
pub mod pending;
pub mod platform;
pub mod policy;
pub mod rng;
pub mod selftest;
pub mod storage;
