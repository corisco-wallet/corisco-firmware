//! LilyGo T-Display-S3 firmware: unlocks the seed through the on-screen flow, then serves BLE
//! signing requests gated by on-device confirmation.

mod display;
mod touch;
mod ui;

use firmware_core::{ble, platform, selftest, storage};

// `AppWindow`, generated from src/ui/app.slint by build.rs.
slint::include_modules!();

use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;
use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let nvs = esp_idf_svc::nvs::EspDefaultNvsPartition::take()?;

    info!("esp32-lilygo-t-display-s3-firmware: boot ok, bringing up the display...");

    let display = match display::Display::new() {
        Ok(disp) => Some(disp),
        Err(e) => {
            log::error!("display: init failed: {e:?}");
            None
        }
    };
    let mut touch = match touch::Touch::new(
        peripherals.i2c0,
        peripherals.pins.gpio18,
        peripherals.pins.gpio17,
        peripherals.pins.gpio16,
        peripherals.pins.gpio21,
    ) {
        Ok(t) => Some(t),
        Err(e) => {
            log::error!("touch: init failed: {e:?}");
            None
        }
    };

    let mut window = None;
    let mut app_ui = None;
    let mut storage_handle: Option<std::rc::Rc<storage::Storage>> = None;
    let onboarded_roots = if let (Some(display), Some(touch)) = (display.as_ref(), touch.as_mut()) {
        let w = platform::EspPlatform::init(display::WIDTH, display::HEIGHT);
        let storage = std::rc::Rc::new(storage::Storage::init(nvs.clone()).expect("NVS storage should initialize"));
        let already_provisioned = storage.is_provisioned().unwrap_or(false);
        info!("ui: starting {} flow", if already_provisioned { "unlock" } else { "onboarding" });
        let result = match ui::run_onboarding_or_unlock(&w, display, touch, storage.clone(), already_provisioned) {
            Ok((ui, seed)) => {
                let roots = corisco_crypto_core::SparkKeyRoots::from_seed(&seed, 0)
                    .expect("key derivation should not fail for this seed");
                info!("ui: flow complete, identity pubkey: {}", hex::encode(roots.identity.public_key_compressed()));
                app_ui = Some(ui);
                Some(roots)
            }
            Err(e) => {
                log::error!("ui: onboarding/unlock flow failed: {e:?}");
                None
            }
        };
        window = Some(w);
        storage_handle = Some(storage);
        result
    } else {
        None
    };

    // Never sign without a real onboarded seed: halt rather than fall back to any key.
    let Some(roots) = onboarded_roots else {
        log::error!("no onboarded seed available (display/touch unavailable, or the onboarding/unlock flow failed) -- halting rather than signing with a fake key");
        loop {
            esp_idf_svc::hal::delay::FreeRtos::delay_ms(1000);
        }
    };

    selftest::bench_sha512_single_block();

    let digest = {
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(b"hello from the T-Display-S3");
        let arr: [u8; 32] = d.into();
        arr
    };
    let sig = corisco_crypto_core::sign_ecdsa_prehashed(&roots.identity.private_key, &digest);
    info!("test signature: {}", hex::encode(sig.to_bytes()));

    selftest::frost_self_test(&roots).expect("FROST self-test should produce a valid signature");

    info!(
        "heap before BLE spawn: free={} largest_free_block={}",
        unsafe { esp_idf_svc::sys::esp_get_free_heap_size() },
        unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(esp_idf_svc::sys::MALLOC_CAP_INTERNAL,) }
    );
    let (passkey_tx, passkey_rx) = mpsc::channel();
    let (confirm_tx, confirm_rx) = mpsc::channel::<Box<ble::SignConfirmationRequest>>();
    let (work_tx, work_rx) = mpsc::channel::<ble::DeferredRequest>();
    let (status_tx, status_rx) = mpsc::channel::<ble::BleStatus>();
    let (pairing_done_tx, pairing_done_rx) = mpsc::channel::<()>();
    // Shared with the BLE thread so the Settings toggle applies to the next `Sign`, no reboot.
    let require_confirmation = Arc::new(AtomicBool::new(
        storage_handle.as_ref().and_then(|s| s.get_require_confirmation().ok()).unwrap_or(true),
    ));
    // 16KB: the default stack crashes BT controller init (a misleading spinlock assertion), and a
    // bigger one eats into the ~90-100KB a live BLE connection needs from tight internal RAM.
    {
        let require_confirmation = require_confirmation.clone();
        std::thread::Builder::new()
            .stack_size(16384)
            .spawn(move || {
                if let Err(e) =
                    ble::run(roots, passkey_tx, confirm_tx, work_tx, status_tx, require_confirmation, pairing_done_tx)
                {
                    log::error!("ble: run failed: {e:?}");
                }
            })
            .expect("failed to spawn BLE thread");
    }

    match (window, app_ui, display.as_ref(), touch.as_mut(), storage_handle) {
        (Some(window), Some(ui), Some(display), Some(touch), Some(storage)) => {
            let pending_sign: std::rc::Rc<std::cell::RefCell<Option<Box<ble::SignConfirmationRequest>>>> =
                std::rc::Rc::new(std::cell::RefCell::new(None));
            {
                let pending_sign = pending_sign.clone();
                let ui_weak = ui.as_weak();
                ui.on_sign_accept_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    if let Some(req) = pending_sign.borrow_mut().take() {
                        ble::complete_sign(req, true);
                    }
                    ui.set_screen(3);
                });
            }
            {
                let pending_sign = pending_sign.clone();
                let ui_weak = ui.as_weak();
                ui.on_sign_decline_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    if let Some(req) = pending_sign.borrow_mut().take() {
                        ble::complete_sign(req, false);
                    }
                    ui.set_screen(3);
                });
            }

            // Factory reset reboots instead of resetting in-process: the BLE thread already holds
            // the keys in RAM. Bonds are kept; a bonded phone still can't sign without on-device confirmation.
            let factory_reset_pending = std::rc::Rc::new(std::cell::Cell::new(false));
            {
                let ui_weak = ui.as_weak();
                ui.on_factory_reset_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    ui.set_screen(10);
                });
            }
            {
                let ui_weak = ui.as_weak();
                ui.on_factory_reset_cancel_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    ui.set_screen(11);
                });
            }
            {
                let ui_weak = ui.as_weak();
                let factory_reset_pending = factory_reset_pending.clone();
                ui.on_factory_reset_confirm_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    factory_reset_pending.set(true);
                    ui.set_working_label("Wiping...".into());
                    ui.set_screen(6);
                });
            }

            {
                let ui_weak = ui.as_weak();
                let require_confirmation = require_confirmation.clone();
                ui.on_settings_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    ui.set_require_confirmation(require_confirmation.load(Ordering::Relaxed));
                    ui.set_screen(11);
                });
            }
            {
                let ui_weak = ui.as_weak();
                ui.on_settings_back_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    ui.set_screen(3);
                });
            }
            {
                // Disabling confirmation asks twice (screen 12); re-enabling is never gated.
                let ui_weak = ui.as_weak();
                let require_confirmation = require_confirmation.clone();
                let storage = storage.clone();
                ui.on_require_confirmation_toggle_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    if require_confirmation.load(Ordering::Relaxed) {
                        ui.set_screen(12);
                    } else {
                        require_confirmation.store(true, Ordering::Relaxed);
                        if let Err(e) = storage.set_require_confirmation(true) {
                            log::error!("main: storage.set_require_confirmation failed: {e:?}");
                        }
                        ui.set_require_confirmation(true);
                    }
                });
            }
            {
                let ui_weak = ui.as_weak();
                ui.on_disable_confirmation_cancel_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    ui.set_screen(11);
                });
            }
            {
                let ui_weak = ui.as_weak();
                let require_confirmation = require_confirmation.clone();
                let storage = storage.clone();
                ui.on_disable_confirmation_confirm_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    require_confirmation.store(false, Ordering::Relaxed);
                    if let Err(e) = storage.set_require_confirmation(false) {
                        log::error!("main: storage.set_require_confirmation failed: {e:?}");
                    }
                    ui.set_require_confirmation(false);
                    ui.set_screen(11);
                });
            }

            let mut last_position = None;
            loop {
                platform::dispatch_touch(&window, touch, &mut last_position);
                slint::platform::update_timers_and_animations();
                platform::render_frame(&window, display);

                // After the render, so the "Wiping..." frame is drawn before restart().
                if factory_reset_pending.get() {
                    if let Err(e) = storage.wipe() {
                        log::error!("main: storage.wipe failed during factory reset: {e:?}");
                    }
                    esp_idf_hal::reset::restart();
                }

                if let Ok(ble::PasskeyDisplay { passkey }) = passkey_rx.try_recv() {
                    ui.set_pairing_passkey(format!("{passkey:06}").into());
                    ui.set_screen(7);
                }

                if let Ok(status) = status_rx.try_recv() {
                    ui.set_ble_connected(matches!(status, ble::BleStatus::Connected));
                }

                // Only from the pairing screen, so a late signal can't steal another screen.
                if pairing_done_rx.try_recv().is_ok() && ui.get_screen() == 7 {
                    ui.set_screen(3);
                }

                if let Ok(req) = confirm_rx.try_recv() {
                    ui.set_sign_leaf_id_short(short_id(&req.leaf_id).into());
                    ui.set_sign_fingerprint(fingerprint(&req.message).into());
                    // Empty means "not supplied"; the UI falls back to leaf id and fingerprint.
                    ui.set_sign_amount_sats(req.amount_sats.map(|a| a.to_string()).unwrap_or_default().into());
                    ui.set_sign_destination(req.destination.clone().unwrap_or_default().into());
                    *pending_sign.borrow_mut() = Some(req);
                    ui.set_screen(4);
                }

                // Blocks the render loop briefly (worst case ~0.5s for a FROST commit).
                if let Ok(req) = work_rx.try_recv() {
                    ble::run_deferred(req);
                }

                esp_idf_svc::hal::delay::FreeRtos::delay_ms(20);
            }
        }
        _ => loop {
            esp_idf_svc::hal::delay::FreeRtos::delay_ms(1000);
        },
    }
}

/// Shortens a leaf UUID to fit the confirm screen.
fn short_id(s: &str) -> String {
    if s.len() <= 13 {
        s.to_string()
    } else {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    }
}

/// Short hex fingerprint of the signed message, shown when the app sends no amount/destination.
fn fingerprint(message: &[u8]) -> String {
    let h = hex::encode(message);
    if h.len() <= 17 {
        h
    } else {
        format!("{}…{}", &h[..8], &h[h.len() - 8..])
    }
}
