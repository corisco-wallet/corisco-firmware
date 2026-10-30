//! Board-agnostic firmware logic shared by every board crate.

pub mod ble;
pub mod pending;
pub mod platform;
pub mod policy;
pub mod rng;
pub mod selftest;
pub mod storage;
