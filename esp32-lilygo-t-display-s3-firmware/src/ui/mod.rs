//! Onboarding and unlock flow: drives `AppWindow` until a decrypted seed is available.
//!
//! Slow crypto (seed derivation, PIN encrypt/decrypt) takes seconds and Slint callbacks block
//! rendering, so callbacks only set a `Stage::Pending*`; the loop runs the work once the
//! "Working..." frame has been drawn. The PIN never reaches Slint, only its digit count.

use crate::display::Display;
use crate::touch::Touch;
use crate::AppWindow;
use firmware_core::storage::{Storage, MAX_PIN_ATTEMPTS};
use firmware_core::{ble, platform, rng};
use corisco_crypto_core::{mnemonic_gen, seed_lock};
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

const PIN_LENGTH: usize = 6;

/// Which path led to the shared `Stage::WordCount`.
#[derive(Clone, Copy)]
enum Purpose {
    Create,
    Recover,
}

enum Stage {
    Welcome,
    WordCount { purpose: Purpose },
    Mnemonic { words: Vec<String>, seed: [u8; 64], page: usize },
    /// `words` are complete wordlist entries; `prefix` is the word in progress.
    WordEntry { words: Vec<String>, total: usize, prefix: String },
    SetPinFirst { seed: [u8; 64] },
    SetPinConfirm { seed: [u8; 64], first_pin: String },
    Unlock,
    /// Terminal: caller should return this seed.
    Done([u8; 64]),
    /// Terminal: nothing to do until a reboot.
    Wiped,
    PendingGenerateMnemonic { word_count: usize },
    PendingEncrypt { seed: [u8; 64], pin: String },
    PendingDecrypt { pin: String },
    PendingValidateRecovery { words: Vec<String> },
}

struct FlowInner {
    stage: Stage,
    pin_buf: String,
}

fn words_model(words: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(words.iter().map(|w| SharedString::from(w.as_str())).collect::<Vec<_>>()))
}

fn candidates_for(prefix: &str) -> Vec<&'static str> {
    mnemonic_gen::wordlist().iter().copied().filter(|w| w.starts_with(prefix)).collect()
}

/// Shared with `on_suggestion_pressed` so a tapped index resolves to the word that was shown.
fn top_suggestions(candidates: &[&'static str]) -> Vec<&'static str> {
    candidates.iter().take(3).copied().collect()
}

fn suggestions_model(words: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(words.iter().map(|w| SharedString::from(*w)).collect::<Vec<_>>()))
}

/// Per letter a-z: whether some candidate continues the prefix with it.
fn letter_enabled_model(candidates: &[&str], prefix_len: usize) -> ModelRc<bool> {
    let mut flags = [false; 26];
    for word in candidates {
        if let Some(&byte) = word.as_bytes().get(prefix_len) {
            if byte.is_ascii_lowercase() {
                flags[(byte - b'a') as usize] = true;
            }
        }
    }
    ModelRc::new(VecModel::from(flags.to_vec()))
}

/// Renders the current stage onto `ui`.
fn sync_ui(ui: &AppWindow, flow: &FlowInner) {
    match &flow.stage {
        Stage::Welcome => ui.set_screen(0),
        Stage::WordCount { .. } => {
            // Keeps `word-entry-error`: a failed recovery lands here and must still show it.
            ui.set_screen(8);
        }
        Stage::Mnemonic { words, page, .. } => {
            ui.set_mnemonic_words(words_model(words));
            ui.set_mnemonic_page(*page as i32);
            ui.set_mnemonic_page_count(words.len().div_ceil(12) as i32);
            ui.set_screen(1);
        }
        Stage::WordEntry { words, total, prefix } => {
            ui.set_word_entry_progress(format!("Word {} of {}", words.len() + 1, total).into());
            ui.set_word_entry_prefix(prefix.as_str().into());
            let candidates = candidates_for(prefix);
            ui.set_word_suggestions(suggestions_model(&top_suggestions(&candidates)));
            ui.set_letter_enabled(letter_enabled_model(&candidates, prefix.len()));
            ui.set_screen(9);
        }
        Stage::SetPinFirst { .. } => {
            ui.set_pin_title("Set a PIN".into());
            ui.set_pin_error("".into());
            ui.set_pin_entered_count(flow.pin_buf.len() as i32);
            ui.set_screen(2);
        }
        Stage::SetPinConfirm { .. } => {
            ui.set_pin_title("Confirm PIN".into());
            ui.set_pin_entered_count(flow.pin_buf.len() as i32);
            ui.set_screen(2);
        }
        Stage::Unlock => {
            ui.set_pin_title("Enter PIN".into());
            ui.set_pin_entered_count(flow.pin_buf.len() as i32);
            ui.set_screen(2);
        }
        Stage::Done(seed) => {
            let pubkey_short = match corisco_crypto_core::SparkKeyRoots::from_seed(seed, 0) {
                Ok(roots) => {
                    let full = hex::encode(roots.identity.public_key_compressed());
                    format!("{}...{}", &full[..8], &full[full.len() - 8..])
                }
                Err(_) => String::new(),
            };
            ui.set_identity_pubkey_short(pubkey_short.into());
            ui.set_screen(3);
        }
        Stage::Wiped => ui.set_screen(5),
        Stage::PendingGenerateMnemonic { .. } => {
            ui.set_working_label("Generating wallet...".into());
            ui.set_screen(6);
        }
        Stage::PendingEncrypt { .. } => {
            ui.set_working_label("Encrypting...".into());
            ui.set_screen(6);
        }
        Stage::PendingDecrypt { .. } => {
            ui.set_working_label("Unlocking...".into());
            ui.set_screen(6);
        }
        Stage::PendingValidateRecovery { .. } => {
            ui.set_working_label("Verifying recovery phrase...".into());
            ui.set_screen(6);
        }
    }
}

/// Runs the slow work for a `Stage::Pending*` and returns the next stage.
fn run_pending_work(ui: &AppWindow, stage: Stage, storage: &Storage) -> Stage {
    match stage {
        Stage::PendingGenerateMnemonic { word_count } => {
            let mnemonic = if word_count >= 24 {
                mnemonic_gen::generate_mnemonic_from_entropy(&rng::device_entropy_256())
            } else {
                mnemonic_gen::generate_mnemonic_from_entropy(&rng::device_entropy_128())
            };
            let t0 = Instant::now();
            let seed = corisco_crypto_core::mnemonic_to_seed(&mnemonic, "");
            log::info!("ui: mnemonic_to_seed took {:?}", t0.elapsed());
            let words: Vec<String> = mnemonic
                .to_string()
                .split_whitespace()
                .enumerate()
                .map(|(i, w)| format!("{}. {w}", i + 1))
                .collect();
            Stage::Mnemonic { words, seed, page: 0 }
        }
        Stage::PendingEncrypt { seed, pin } => {
            let t0 = Instant::now();
            let enc = seed_lock::encrypt_seed_with_randomness(&seed, &pin, rng::random_salt_16(), rng::random_nonce_12());
            log::info!("ui: encrypt_seed_with_randomness took {:?}", t0.elapsed());
            match storage.store(&enc) {
                Ok(()) => Stage::Done(seed),
                Err(e) => {
                    log::error!("ui: storage.store failed: {e:?}");
                    ui.set_pin_error("Storage error, try again".into());
                    Stage::SetPinFirst { seed }
                }
            }
        }
        Stage::PendingDecrypt { pin } => match storage.load() {
            Ok(Some(enc)) => {
                let t0 = Instant::now();
                let result = seed_lock::decrypt_seed(&enc, &pin);
                log::info!("ui: decrypt_seed took {:?}", t0.elapsed());
                match result {
                    Ok(seed) => {
                        let _ = storage.reset_failures();
                        Stage::Done(seed)
                    }
                    Err(_) => match storage.record_failure() {
                        Ok(count) if count >= MAX_PIN_ATTEMPTS => {
                            if let Err(e) = storage.wipe() {
                                log::error!("ui: storage.wipe failed: {e:?}");
                            }
                            Stage::Wiped
                        }
                        Ok(count) => {
                            ui.set_pin_error(format!("Wrong PIN, {} attempts left", MAX_PIN_ATTEMPTS - count).into());
                            Stage::Unlock
                        }
                        Err(e) => {
                            log::error!("ui: storage.record_failure failed: {e:?}");
                            ui.set_pin_error("Storage error, try again".into());
                            Stage::Unlock
                        }
                    },
                }
            }
            Ok(None) => {
                log::error!("ui: Unlock reached with no stored seed blob");
                ui.set_pin_error("No wallet found".into());
                Stage::Unlock
            }
            Err(e) => {
                log::error!("ui: storage.load failed: {e:?}");
                ui.set_pin_error("Storage error, try again".into());
                Stage::Unlock
            }
        },
        Stage::PendingValidateRecovery { words } => match mnemonic_gen::parse_mnemonic(&words) {
            Ok(mnemonic) => {
                let t0 = Instant::now();
                let seed = corisco_crypto_core::mnemonic_to_seed(&mnemonic, "");
                log::info!("ui: mnemonic_to_seed (recovery) took {:?}", t0.elapsed());
                Stage::SetPinFirst { seed }
            }
            Err(e) => {
                // Every word is valid, so this is a checksum failure, which can't say which word
                // is wrong: restart recovery.
                log::warn!("ui: recovery phrase failed validation: {e}");
                ui.set_word_entry_error("Recovery phrase invalid, try again".into());
                Stage::WordCount { purpose: Purpose::Recover }
            }
        },
        other => other,
    }
}

/// Handles a full PIN buffer; slow crypto is deferred via `Stage::Pending*`.
fn submit_pin(ui: &AppWindow, flow: &mut FlowInner) {
    let pin = std::mem::take(&mut flow.pin_buf);
    flow.stage = match std::mem::replace(&mut flow.stage, Stage::Welcome) {
        Stage::SetPinFirst { seed } => Stage::SetPinConfirm { seed, first_pin: pin },
        Stage::SetPinConfirm { seed, first_pin } => {
            if pin == first_pin {
                Stage::PendingEncrypt { seed, pin }
            } else {
                ui.set_pin_error("PINs didn't match, try again".into());
                Stage::SetPinFirst { seed }
            }
        }
        Stage::Unlock => Stage::PendingDecrypt { pin },
        other => other,
    };
}

/// Blocks until a seed is ready and returns it with the `AppWindow`, which main.rs keeps using.
/// Never returns once the device is wiped.
pub fn run_onboarding_or_unlock(
    window: &Rc<MinimalSoftwareWindow>,
    display: &Display,
    touch: &mut Touch,
    storage: Rc<Storage>,
    already_provisioned: bool,
) -> anyhow::Result<(AppWindow, [u8; 64])> {
    let ui = AppWindow::new().map_err(|e| anyhow::anyhow!("AppWindow::new failed: {e:?}"))?;
    ui.set_device_name(ble::device_name().into());
    ui.show().map_err(|e| anyhow::anyhow!("AppWindow::show failed: {e:?}"))?;

    let flow = Rc::new(RefCell::new(FlowInner {
        stage: if already_provisioned { Stage::Unlock } else { Stage::Welcome },
        pin_buf: String::new(),
    }));
    sync_ui(&ui, &flow.borrow());

    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_create_pressed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            flow.stage = Stage::WordCount { purpose: Purpose::Create };
            ui.set_word_entry_error("".into());
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_recover_pressed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            flow.stage = Stage::WordCount { purpose: Purpose::Recover };
            ui.set_word_entry_error("".into());
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_word_count_pressed(move |count| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            let word_count = count.max(0) as usize;
            flow.stage = match std::mem::replace(&mut flow.stage, Stage::Welcome) {
                Stage::WordCount { purpose: Purpose::Create } => Stage::PendingGenerateMnemonic { word_count },
                Stage::WordCount { purpose: Purpose::Recover } => {
                    Stage::WordEntry { words: Vec::new(), total: word_count, prefix: String::new() }
                }
                other => other,
            };
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_mnemonic_continue_pressed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            flow.stage = match std::mem::replace(&mut flow.stage, Stage::Welcome) {
                Stage::Mnemonic { seed, .. } => Stage::SetPinFirst { seed },
                other => other,
            };
            flow.pin_buf.clear();
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_mnemonic_next_page_pressed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            if let Stage::Mnemonic { page, .. } = &mut flow.stage {
                *page += 1;
            }
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_mnemonic_prev_page_pressed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            if let Stage::Mnemonic { page, .. } = &mut flow.stage {
                *page = page.saturating_sub(1);
            }
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_letter_pressed(move |idx| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            if let Stage::WordEntry { prefix, .. } = &mut flow.stage {
                if (0..26).contains(&idx) {
                    prefix.push((b'a' + idx as u8) as char);
                }
            }
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_suggestion_pressed(move |idx| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            flow.stage = match std::mem::replace(&mut flow.stage, Stage::Welcome) {
                Stage::WordEntry { mut words, total, prefix } => {
                    let candidates = candidates_for(&prefix);
                    let suggestions = top_suggestions(&candidates);
                    match suggestions.get(idx.max(0) as usize) {
                        Some(word) => {
                            words.push((*word).to_string());
                            if words.len() >= total {
                                Stage::PendingValidateRecovery { words }
                            } else {
                                Stage::WordEntry { words, total, prefix: String::new() }
                            }
                        }
                        None => Stage::WordEntry { words, total, prefix },
                    }
                }
                other => other,
            };
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_word_entry_backspace_pressed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            if let Stage::WordEntry { words, prefix, .. } = &mut flow.stage {
                if !prefix.is_empty() {
                    prefix.pop();
                } else if let Some(last) = words.pop() {
                    *prefix = last;
                }
            }
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_pin_digit_pressed(move |digit| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            if flow.pin_buf.len() < PIN_LENGTH {
                flow.pin_buf.push_str(&digit.to_string());
            }
            if flow.pin_buf.len() == PIN_LENGTH {
                submit_pin(&ui, &mut flow);
                flow.pin_buf.clear();
            }
            sync_ui(&ui, &flow);
        });
    }
    {
        let flow = flow.clone();
        let ui_weak = ui.as_weak();
        ui.on_pin_backspace_pressed(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut flow = flow.borrow_mut();
            flow.pin_buf.pop();
            sync_ui(&ui, &flow);
        });
    }

    let mut last_position = None;
    loop {
        platform::dispatch_touch(window, touch, &mut last_position);
        slint::platform::update_timers_and_animations();
        platform::render_frame(window, display);

        // After the render, so the Home screen set last iteration is drawn before returning.
        if let Stage::Done(seed) = &flow.borrow().stage {
            return Ok((ui, *seed));
        }

        // Only once the "Working..." frame has been drawn.
        let is_pending = matches!(
            flow.borrow().stage,
            Stage::PendingGenerateMnemonic { .. }
                | Stage::PendingEncrypt { .. }
                | Stage::PendingDecrypt { .. }
                | Stage::PendingValidateRecovery { .. }
        );
        if is_pending {
            let mut flow = flow.borrow_mut();
            let taken = std::mem::replace(&mut flow.stage, Stage::Welcome);
            flow.stage = run_pending_work(&ui, taken, &storage);
            flow.pin_buf.clear();
            sync_ui(&ui, &flow);
        }

        esp_idf_svc::hal::delay::FreeRtos::delay_ms(20);
    }
}
