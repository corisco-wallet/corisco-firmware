//! Spark hardware signer firmware: display + touch + Slint UI,
//! encrypted-at-rest seed storage behind a PIN, and a BLE GATT signing
//! service gated on an on-screen accept/decline.
//!
//! Boot sequence: bring up the display/touch (isolated milestones, log-
//! only on failure) -> run the onboarding-or-unlock Slint flow to get a
//! real seed into RAM (`ui::run_onboarding_or_unlock`) -> spawn BLE on
//! its own thread (`ble::run`) -> the main thread's loop keeps rendering/
//! dispatching touch for whatever's showing (Home, a pairing passkey, or
//! a sign-confirmation prompt) for the rest of the device's uptime.
//!
//! Known limitation: BLE's wire protocol doesn't always carry a
//! human-readable amount/destination for every signing request, so the
//! confirmation screen falls back to a leaf id + message fingerprint when
//! neither is supplied (a deliberate MVP scope decision, not an oversight
//! -- see `ble.rs`'s `Request::Sign` doc comment).

mod display;
mod touch;
mod ui;

use firmware_core::{ble, platform, selftest, storage};

// Brings in `AppWindow`, generated from `src/ui/app.slint` by
// `slint_build` in build.rs.
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
    // Taken once here, not independently in both `storage::init` and
    // `ble::run` -- `EspDefaultNvsPartition::take()` is a process-wide
    // singleton; a second call after the first succeeds returns an error.
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

    // `onboarded_roots` stays `None` if display/touch aren't available or
    // the flow fails -- in that case main() halts below rather than
    // starting BLE with no real seed (see the
    // `Some(roots) = onboarded_roots else { ... }` check further down).
    let mut window = None;
    let mut app_ui = None;
    // Hoisted out of the `if let` block below (not just used inside
    // `run_onboarding_or_unlock`) -- the factory-reset wiring further
    // down, on the Home screen's own loop, needs this same `Storage`
    // handle to actually erase the seed blob.
    let mut storage_handle: Option<std::rc::Rc<storage::Storage>> = None;
    let onboarded_roots = if let (Some(display), Some(touch)) = (display.as_ref(), touch.as_mut()) {
        let w = platform::EspPlatform::init(display::WIDTH, display::HEIGHT);
        let storage = std::rc::Rc::new(
            storage::Storage::init(nvs.clone()).expect("NVS storage should initialize"),
        );
        let already_provisioned = storage.is_provisioned().unwrap_or(false);
        info!(
            "ui: starting {} flow",
            if already_provisioned { "unlock" } else { "onboarding" }
        );
        let result = match ui::run_onboarding_or_unlock(&w, display, touch, storage.clone(), already_provisioned) {
            Ok((ui, seed)) => {
                let roots = corisco_crypto_core::SparkKeyRoots::from_seed(&seed, 0)
                    .expect("key derivation should not fail for this seed");
                info!(
                    "ui: flow complete, identity pubkey: {}",
                    hex::encode(roots.identity.public_key_compressed())
                );
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

    // BLE only ever signs with a real, onboarded seed -- there is no
    // hardcoded fallback key. Without display/touch (or a failed
    // onboarding/unlock flow), there is no seed and nothing safe to do;
    // halt rather than ever fall back to a fake key.
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

    // BLE runs on its own thread (`ble::run`, esp32-nimble). `BLEDevice::take()`
    // is a bare lazy-static singleton with no Rust-level peripheral handle
    // to thread through, so this spawn needs nothing from `peripherals`/`nvs`.
    // Note: the default `std::thread` stack size is NOT enough here -- it
    // crashes BT controller init with
    // `assert failed: spinlock_acquire spinlock.h:142 (lock->count == 0)`,
    // an assertion that looks unrelated to stack size on its face. An
    // explicit stack size (below) is required.
    info!(
        "heap before BLE spawn: free={} largest_free_block={}",
        unsafe { esp_idf_svc::sys::esp_get_free_heap_size() },
        unsafe {
            esp_idf_svc::sys::heap_caps_get_largest_free_block(
                esp_idf_svc::sys::MALLOC_CAP_INTERNAL,
            )
        }
    );
    let (passkey_tx, passkey_rx) = mpsc::channel();
    // `Sign` requests arrive here instead of being answered inline --
    // `ble::complete_sign` (called below, from Accept/Decline) is what
    // actually produces the response.
    let (confirm_tx, confirm_rx) = mpsc::channel::<Box<ble::SignConfirmationRequest>>();
    // `Commit`/`GetLeafPublicKey`/`SignSchnorrIdentity`/`SignEcdsaIdentity`
    // all do fresh elliptic-curve math that can't run inline in `on_write`
    // -- it overflowed the NimBLE host task's stack on real hardware (see
    // ble.rs's `handle_request` doc comment) -- so they're deferred here
    // too, completed on this thread's render loop via `ble::run_deferred`.
    let (work_tx, work_rx) = mpsc::channel::<ble::DeferredRequest>();
    // Home screen's BLE indicator ("Advertising" vs "Connected") -- see
    // ble.rs's `BleStatus` doc comment.
    let (status_tx, status_rx) = mpsc::channel::<ble::BleStatus>();
    // Signals leaving the pairing-passkey screen (7) once bonding actually
    // completes -- see ble.rs's `run` doc comment for the bug this fixes
    // (nothing previously told the UI to leave that screen at all).
    let (pairing_done_tx, pairing_done_rx) = mpsc::channel::<()>();
    // Settings screen's "require confirmation" toggle -- shared with
    // `ble::run`'s `Signer` so flipping it (from the Home/Settings loop
    // below) takes effect on the very next `Sign`, no reboot needed.
    // Seeded from storage here, kept in sync with it by the settings
    // wiring further down. `storage_handle` is guaranteed `Some` at this
    // point (display/touch/storage were all set together, and `roots`
    // above already confirmed that path succeeded).
    let require_confirmation = Arc::new(AtomicBool::new(
        storage_handle.as_ref().and_then(|s| s.get_require_confirmation().ok()).unwrap_or(true),
    ));
    // Total internal RAM is tight (~150KB free/~90KB largest contiguous
    // block right before this point, per the heap log above), and a real
    // BLE connection's own internal overhead (GATT tables, bonding/
    // security state, MTU-256 buffers) costs ~90-100KB by itself. This
    // thread's stack is kept small (16384 bytes) specifically to leave
    // that headroom: only the synchronous setup in `ble::run` itself
    // (BLEDevice::take, security/service/characteristic setup, advertising
    // start) runs here -- the deep FROST/postcard call chain in
    // `on_write`'s closure runs on the NimBLE host task's own stack, not
    // this one (see `sdkconfig.defaults`'s stack-size settings for that
    // side of the budget). Don't "free up" this headroom by shrinking
    // `CONFIG_ESP_MAIN_TASK_STACK_SIZE` instead -- the FROST/ECDSA
    // self-test's elliptic-curve math needs real stack depth there, on
    // every boot.
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

    // Keep rendering/dispatching touch so the pairing-passkey and sign-
    // confirmation screens actually show up. No display/touch/window
    // means onboarding's UI failed earlier; just idle instead, matching
    // this firmware's original headless behavior.
    match (window, app_ui, display.as_ref(), touch.as_mut(), storage_handle) {
        (Some(window), Some(ui), Some(display), Some(touch), Some(storage)) => {
            // Holds the request currently on screen 4 (SignConfirm) --
            // `None` until one arrives, taken (and answered) when the
            // user taps Accept/Decline.
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

            // Factory reset (screen 10, from Home/screen 3): a deliberate
            // reboot, not an in-process reset back to Welcome -- by the
            // time Home is showing, the BLE thread is already signing
            // with the current in-RAM `SparkKeyRoots`, and wiping
            // `storage`'s NVS blob doesn't change what's already in RAM
            // or stop BLE from continuing to use it. A reboot makes every
            // piece of this firmware's state start genuinely fresh
            // (`roots`, the BLE thread, `pending_commitments`, etc.)
            // instead of needing to individually tear each of them down.
            // Deliberately leaves BLE bonding data untouched -- re-pairing
            // every previously-bonded phone after every reset would be
            // poor UX, and a bonded phone still can't sign anything
            // without this device's own on-screen confirmation regardless
            // of bond state.
            //
            // `factory_reset_pending` mirrors `ui/mod.rs`'s
            // `Stage::Pending*` split (render the "Wiping…" frame first,
            // only then do the real thing) -- Home's loop lives here, in
            // `main.rs`, not in that module's state machine, so the same
            // technique is reimplemented locally instead of reusing it
            // directly.
            let factory_reset_pending = std::rc::Rc::new(std::cell::Cell::new(false));
            {
                let ui_weak = ui.as_weak();
                ui.on_factory_reset_pressed(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    ui.set_screen(10);
                });
            }
            {
                // Back to Settings (11), not Home -- factory reset is now
                // reached from the Settings screen, not directly from
                // Home (see `on_settings_pressed` below).
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

            // Settings (11): currently just the "require confirmation"
            // toggle plus the entry point into factory reset (moved here
            // from being a direct Home-screen link -- this is where
            // device-level settings belong as more of them show up).
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
                // Turning confirmation OFF is a real security-posture
                // change -- gated behind screen 12's explicit warning,
                // same "make the destructive direction ask twice" shape
                // as factory reset. Turning it back ON needs no gate --
                // making things safer again should never be the hard
                // path.
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

                // Checked right after the render above, same "let the
                // Working/Wiping frame actually draw first" ordering
                // `ui/mod.rs`'s own pending-work check uses -- `restart()`
                // never returns, so this only ever runs once.
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

                // Leave the pairing-passkey screen once bonding actually
                // completes -- only if that's still what's showing, so
                // this can never steal the screen away from something
                // else (a sign-confirm prompt, Settings, etc.) if this
                // ever fired at an unexpected time.
                if pairing_done_rx.try_recv().is_ok() && ui.get_screen() == 7 {
                    ui.set_screen(3);
                }

                if let Ok(req) = confirm_rx.try_recv() {
                    ui.set_sign_leaf_id_short(short_id(&req.leaf_id).into());
                    ui.set_sign_fingerprint(fingerprint(&req.message).into());
                    // Empty string means "not supplied" to the UI (see
                    // app.slint's sign-amount-sats doc comment) -- falls
                    // back to the leaf id/fingerprint view above.
                    ui.set_sign_amount_sats(
                        req.amount_sats.map(|a| a.to_string()).unwrap_or_default().into(),
                    );
                    ui.set_sign_destination(req.destination.clone().unwrap_or_default().into());
                    *pending_sign.borrow_mut() = Some(req);
                    ui.set_screen(4);
                }

                // No screen change (unlike Sign) -- none of these need user
                // confirmation, just a stack big enough to run on. Blocks
                // this render loop briefly (worst case one FROST round1,
                // ~0.5s per the boot-time self-test) -- a hitch, not a hang.
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

/// Shortens a leaf id (a UUID string) for the sign-confirmation screen --
/// the full id doesn't fit and isn't more meaningful to a human than a
/// short prefix/suffix.
fn short_id(s: &str) -> String {
    if s.len() <= 13 {
        s.to_string()
    } else {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    }
}

/// A short hex fingerprint of the message being signed -- the fallback
/// shown on the confirm screen when a request didn't supply
/// `amount_sats`/`destination` (see `ble.rs`'s `Request::Sign` doc
/// comment). Not proof of what the transaction actually does, just enough
/// for the user to eyeball "this matches what I expected."
fn fingerprint(message: &[u8]) -> String {
    let h = hex::encode(message);
    if h.len() <= 17 {
        h
    } else {
        format!("{}…{}", &h[..8], &h[h.len() - 8..])
    }
}
