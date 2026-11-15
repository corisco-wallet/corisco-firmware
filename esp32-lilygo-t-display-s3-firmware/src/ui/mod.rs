//! Onboarding and unlock flow: drives `AppWindow` until a decrypted seed is available.
//!
//! Slow crypto (seed derivation, PIN encrypt/decrypt) takes seconds and Slint callbacks block
//! rendering, so callbacks only set a `Stage::Pending*`; the loop hands it to a worker thread and
//! keeps rendering the spinner. The PIN never reaches Slint, only its digit count.

use crate::display::Display;
use crate::power::{self, PowerButton};
use crate::touch::Touch;
use crate::worker;
use crate::AppWindow;
use bip39::Mnemonic;
use corisco_crypto_core::seed_lock::{EncryptedSeed, SeedLockError};
use corisco_crypto_core::{mnemonic_gen, seed_lock};
use firmware_core::storage::{Storage, MAX_PIN_ATTEMPTS};
use firmware_core::{ble, platform, rng};
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

const PIN_LENGTH: usize = 6;
/// Generous for PBKDF2 + AES-GCM; allocated only while the worker runs, before BLE needs the RAM.
const CRYPTO_WORKER_STACK: usize = 32 * 1024;

/// Which path led to the shared `Stage::WordCount`.
#[derive(Clone, Copy)]
enum Purpose {
    Create,
    Recover,
}

enum Stage {
    Welcome,
    WordCount {
        purpose: Purpose,
    },
    Mnemonic {
        words: Vec<String>,
        seed: [u8; 64],
        page: usize,
    },
    /// `words` are complete wordlist entries; `prefix` is the word in progress.
    WordEntry {
        words: Vec<String>,
        total: usize,
        prefix: String,
    },
    SetPinFirst {
        seed: [u8; 64],
    },
    SetPinConfirm {
        seed: [u8; 64],
        first_pin: String,
    },
    Unlock,
    /// Terminal: caller should return this seed.
    Done([u8; 64]),
    /// Terminal: nothing to do until a reboot.
    Wiped,
    PendingGenerateMnemonic {
        word_count: usize,
    },
    PendingEncrypt {
        seed: [u8; 64],
        pin: String,
    },
    PendingDecrypt {
        pin: String,
    },
    PendingValidateRecovery {
        words: Vec<String>,
    },
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
            // main.rs switches to Home once its setup is done and the loop is polling touch.
            ui.set_working_label("Loading".into());
            ui.set_screen(6);
        }
        Stage::Wiped => ui.set_screen(5),
        Stage::PendingGenerateMnemonic { .. } => {
            ui.set_working_label("Generating wallet".into());
            ui.set_screen(6);
        }
        Stage::PendingEncrypt { .. } => {
            ui.set_working_label("Encrypting".into());
            ui.set_screen(6);
        }
        Stage::PendingDecrypt { .. } => {
            ui.set_working_label("Unlocking".into());
            ui.set_screen(6);
        }
        Stage::PendingValidateRecovery { .. } => {
            ui.set_working_label("Verifying recovery phrase".into());
            ui.set_screen(6);
        }
    }
}

/// Slow crypto for a `Stage::Pending*`, with its inputs; run on the crypto worker.
enum PendingWork {
    GenerateMnemonic(Mnemonic),
    Encrypt { seed: [u8; 64], pin: String, salt: [u8; 16], nonce: [u8; 12] },
    Decrypt { enc: EncryptedSeed, pin: String },
    ValidateRecovery(Mnemonic),
}

impl PendingWork {
    fn run(self) -> WorkOutput {
        let t0 = Instant::now();
        let output = match self {
            PendingWork::GenerateMnemonic(mnemonic) => {
                let seed = corisco_crypto_core::mnemonic_to_seed(&mnemonic, "");
                let words = mnemonic
                    .to_string()
                    .split_whitespace()
                    .enumerate()
                    .map(|(i, w)| format!("{}. {w}", i + 1))
                    .collect();
                WorkOutput::NewMnemonic { words, seed }
            }
            PendingWork::Encrypt { seed, pin, salt, nonce } => {
                let enc = seed_lock::encrypt_seed_with_randomness(&seed, &pin, salt, nonce);
                WorkOutput::Encrypted { seed, enc }
            }
            PendingWork::Decrypt { enc, pin } => WorkOutput::Decrypted(seed_lock::decrypt_seed(&enc, &pin)),
            PendingWork::ValidateRecovery(mnemonic) => {
                WorkOutput::Recovered { seed: corisco_crypto_core::mnemonic_to_seed(&mnemonic, "") }
            }
        };
        log::info!("ui: pending work took {:?}", t0.elapsed());
        output
    }
}

/// Output of the slow crypto run on the worker; storage writes and UI updates stay on the UI thread.
enum WorkOutput {
    NewMnemonic { words: Vec<String>, seed: [u8; 64] },
    Encrypted { seed: [u8; 64], enc: EncryptedSeed },
    Decrypted(Result<[u8; 64], SeedLockError>),
    Recovered { seed: [u8; 64] },
}

/// Runs `work` on the crypto worker while keeping the Working screen's spinner animated.
///
/// Falls back to running inline (spinner frozen) when internal RAM has no block for the stack.
pub fn run_with_spinner<T: Send + 'static>(
    ui: &AppWindow,
    window: &MinimalSoftwareWindow,
    display: &Display,
    power_button: &mut PowerButton,
    stack_size: usize,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let largest_before = worker::largest_internal_block();
    let spawned = if worker::has_room_for(stack_size) { worker::spawn(stack_size, work) } else { Err(work) };
    let worker = match spawned {
        Ok(worker) => worker,
        Err(work) => return run_inline(ui, window, display, power_button, work),
    };
    while !worker.is_finished() {
        slint::platform::update_timers_and_animations();
        platform::render_frame(window, display);
        if power_button.long_pressed() {
            power::power_off(ui, window, display, power_button);
        }
        esp_idf_svc::hal::delay::FreeRtos::delay_ms(20);
    }
    let output = worker.join();
    // Allocations that outlive the worker can pin its freed stack apart from the rest of its
    // block, leaving too little contiguous RAM for a later BLE connection.
    let largest_after = worker::largest_internal_block();
    if largest_after < largest_before {
        log::warn!("ui: largest internal block shrank from {largest_before} to {largest_after} across the worker");
    }
    output
}

fn run_inline<T>(
    ui: &AppWindow,
    window: &MinimalSoftwareWindow,
    display: &Display,
    power_button: &mut PowerButton,
    work: impl FnOnce() -> T,
) -> T {
    log::warn!("ui: no crypto worker, running inline with the spinner and power button frozen");
    platform::render_frame(window, display);
    let output = work();
    log::info!("ui: ran inline, {} main task stack bytes never used", worker::unused_stack_bytes());
    // The button is level-polled, so a hold that started during the work still powers off here.
    if power_button.long_pressed() {
        power::power_off(ui, window, display, power_button);
    }
    output
}

/// Collects a `Stage::Pending*`'s inputs for the worker, or returns the next stage directly when a
/// fast check (storage load, checksum) already decides it.
fn prepare_pending_work(ui: &AppWindow, stage: Stage, storage: &Storage) -> Result<PendingWork, Stage> {
    match stage {
        Stage::PendingGenerateMnemonic { word_count } => {
            let mnemonic = if word_count >= 24 {
                mnemonic_gen::generate_mnemonic_from_entropy(&rng::device_entropy_256())
            } else {
                mnemonic_gen::generate_mnemonic_from_entropy(&rng::device_entropy_128())
            };
            Ok(PendingWork::GenerateMnemonic(mnemonic))
        }
        Stage::PendingEncrypt { seed, pin } => {
            Ok(PendingWork::Encrypt { seed, pin, salt: rng::random_salt_16(), nonce: rng::random_nonce_12() })
        }
        Stage::PendingDecrypt { pin } => match storage.load() {
            Ok(Some(enc)) => Ok(PendingWork::Decrypt { enc, pin }),
            Ok(None) => {
                log::error!("ui: Unlock reached with no stored seed blob");
                ui.set_pin_error("No wallet found".into());
                Err(Stage::Unlock)
            }
            Err(e) => {
                log::error!("ui: storage.load failed: {e:?}");
                ui.set_pin_error("Storage error, try again".into());
                Err(Stage::Unlock)
            }
        },
        Stage::PendingValidateRecovery { words } => match mnemonic_gen::parse_mnemonic(&words) {
            Ok(mnemonic) => Ok(PendingWork::ValidateRecovery(mnemonic)),
            Err(e) => {
                // Every word is valid, so this is a checksum failure, which can't say which word
                // is wrong: restart recovery.
                log::warn!("ui: recovery phrase failed validation: {e}");
                ui.set_word_entry_error("Recovery phrase invalid, try again".into());
                Err(Stage::WordCount { purpose: Purpose::Recover })
            }
        },
        other => Err(other),
    }
}

/// Applies a finished worker's output and returns the next stage.
fn finish_pending_work(ui: &AppWindow, output: WorkOutput, storage: &Storage) -> Stage {
    match output {
        WorkOutput::NewMnemonic { words, seed } => Stage::Mnemonic { words, seed, page: 0 },
        WorkOutput::Encrypted { seed, enc } => match storage.store(&enc) {
            Ok(()) => Stage::Done(seed),
            Err(e) => {
                log::error!("ui: storage.store failed: {e:?}");
                ui.set_pin_error("Storage error, try again".into());
                Stage::SetPinFirst { seed }
            }
        },
        WorkOutput::Decrypted(Ok(seed)) => {
            let _ = storage.reset_failures();
            Stage::Done(seed)
        }
        WorkOutput::Decrypted(Err(_)) => match storage.record_failure() {
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
        WorkOutput::Recovered { seed } => Stage::SetPinFirst { seed },
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
    power_button: &mut PowerButton,
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

        // After the render, so the Loading screen set last iteration is drawn before returning.
        if let Stage::Done(seed) = &flow.borrow().stage {
            return Ok((ui, *seed));
        }

        if power_button.long_pressed() {
            power::power_off(&ui, window, display, power_button);
        }

        let is_pending = matches!(
            flow.borrow().stage,
            Stage::PendingGenerateMnemonic { .. }
                | Stage::PendingEncrypt { .. }
                | Stage::PendingDecrypt { .. }
                | Stage::PendingValidateRecovery { .. }
        );
        if is_pending {
            let prepared = {
                let mut flow = flow.borrow_mut();
                flow.pin_buf.clear();
                let stage = std::mem::replace(&mut flow.stage, Stage::Welcome);
                prepare_pending_work(&ui, stage, &storage)
            };
            let next = match prepared {
                Ok(job) => {
                    let output =
                        run_with_spinner(&ui, window, display, power_button, CRYPTO_WORKER_STACK, move || job.run());
                    finish_pending_work(&ui, output, &storage)
                }
                Err(next) => next,
            };
            let mut flow = flow.borrow_mut();
            flow.stage = next;
            sync_ui(&ui, &flow);
        }

        esp_idf_svc::hal::delay::FreeRtos::delay_ms(20);
    }
}
