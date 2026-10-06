//! Onboarding/unlock flow controller -- drives `AppWindow` (generated from
//! `app.slint`) through
//! Welcome -> [Create: WordCount -> Mnemonic] or [Recover: WordCount ->
//! WordEntry] -> Set PIN (either path) or Unlock (already provisioned),
//! returning the decrypted 64-byte seed once either path completes. The
//! sign-confirmation screen (`AppWindow`'s `screen == 4`) and the Home
//! screen's factory-reset flow (`screen == 10`) are driven separately,
//! later, by `main.rs`'s own loop once BLE is up -- they reuse this same
//! `AppWindow`/window rather than this module owning it past the point
//! this function returns.
//!
//! PIN digits never touch the UI layer as a real string beyond what's
//! needed to encrypt/decrypt/compare -- `app.slint` only ever receives a
//! digit *count* (for the dot indicator), never the PIN itself.
//!
//! Every real crypto call here (mnemonic/seed generation, PIN encrypt/
//! decrypt, recovery-phrase validation) is genuinely slow -- seconds, not
//! milliseconds, in pure software on this chip (see
//! `corisco_crypto_core::mnemonic_to_seed`'s and `seed_lock::PIN_KDF_ITERATIONS`'s
//! own doc comments). Slint callbacks run synchronously inside
//! `dispatch_touch`, and nothing else redraws the screen until the
//! callback returns -- calling one of these directly from a callback
//! would freeze the whole UI for that whole duration with zero visual
//! feedback, indistinguishable from a crash to the user. So each such
//! transition is split in two: the
//! callback only sets a `Stage::Pending*` marker (fast, renders the
//! "Working…" screen on the very next frame), and the main loop below --
//! *after* that frame has actually been drawn -- performs the real work
//! and applies the resulting stage.
//!
//! Word entry (recovery), by contrast, is cheap string filtering against
//! the in-memory BIP39 wordlist -- no `Pending` split needed for each
//! keystroke, only for the final checksum-validate-and-derive-seed step
//! once all words are in (`Stage::PendingValidateRecovery`).

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

/// Which onboarding path led to `Stage::WordCount` -- it's shared between
/// both (see that stage's doc comment), so something has to remember
/// which follow-up stage to produce once a word count is picked.
#[derive(Clone, Copy)]
enum Purpose {
    Create,
    Recover,
}

enum Stage {
    Welcome,
    /// Shared between the create and recover paths -- `purpose` decides
    /// whether picking a count leads to `PendingGenerateMnemonic` (create)
    /// or `WordEntry` (recover).
    WordCount { purpose: Purpose },
    /// `page` is which 12-word page (0-based) is currently shown --
    /// paginated rather than scrollable (see `app.slint`'s own comment on
    /// this screen): a flick-to-scroll gesture is easy to miss entirely
    /// on a screen this small, and missing a word of your own recovery
    /// phrase is a much worse failure than on any other screen here.
    Mnemonic { words: Vec<String>, seed: [u8; 64], page: usize },
    /// Recovery's word-by-word entry. `words` holds completed words so
    /// far (already validated as real wordlist entries -- see
    /// `letter_enabled_model`, which only ever leaves letters tappable
    /// when they can continue some real word); `total` is 12 or 24 from
    /// `WordCount`; `prefix` is the current word's not-yet-completed
    /// letters.
    WordEntry { words: Vec<String>, total: usize, prefix: String },
    SetPinFirst { seed: [u8; 64] },
    SetPinConfirm { seed: [u8; 64], first_pin: String },
    Unlock,
    /// Terminal: caller should return this seed.
    Done([u8; 64]),
    /// Terminal: loops forever showing the wiped screen (see module doc --
    /// there is genuinely nothing else to do here short of a physical
    /// reboot back into onboarding).
    Wiped,
    /// Set by `on_create_pressed`; the main loop does the real
    /// mnemonic/seed generation once this has rendered.
    PendingGenerateMnemonic { word_count: usize },
    /// Set once a 6th PIN digit lands on `SetPinConfirm`; the main loop
    /// does the real encrypt+store once this has rendered.
    PendingEncrypt { seed: [u8; 64], pin: String },
    /// Set once a 6th PIN digit lands on `Unlock`; the main loop does the
    /// real load+decrypt once this has rendered.
    PendingDecrypt { pin: String },
    /// Set once `WordEntry` collects its final word; the main loop
    /// checksum-validates the phrase and (on success) derives the seed.
    PendingValidateRecovery { words: Vec<String> },
}

struct FlowInner {
    stage: Stage,
    pin_buf: String,
}

fn words_model(words: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(words.iter().map(|w| SharedString::from(w.as_str())).collect::<Vec<_>>()))
}

/// Every wordlist entry starting with `prefix` -- the live candidate set
/// an in-progress word could still resolve to. Recomputed on every
/// keystroke rather than cached: `wordlist()` is a flat 2048-entry array,
/// a linear scan+filter is well under a millisecond, nowhere near the
/// "needs a Pending split" threshold the module doc comment describes.
fn candidates_for(prefix: &str) -> Vec<&'static str> {
    mnemonic_gen::wordlist().iter().copied().filter(|w| w.starts_with(prefix)).collect()
}

/// The (up to 3) candidates actually offered as tap-to-complete
/// suggestions -- `wordlist()` is alphabetically ordered, so this is
/// deterministic and, critically, computed the exact same way here as in
/// `on_suggestion_pressed` (which resolves a tapped index back to a real
/// word using this same function) -- they must never drift apart.
fn top_suggestions(candidates: &[&'static str]) -> Vec<&'static str> {
    candidates.iter().take(3).copied().collect()
}

fn suggestions_model(words: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(words.iter().map(|w| SharedString::from(*w)).collect::<Vec<_>>()))
}

/// One flag per letter a-z: whether any candidate word has that letter
/// immediately after the current prefix -- what `app.slint`'s adaptive
/// keyboard dims/disables against, so the user can never type a prefix
/// that can't possibly complete into a real wordlist entry.
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

/// Re-renders whatever `flow.stage` currently is onto `ui`'s properties.
/// Called once up front and again after every stage transition.
fn sync_ui(ui: &AppWindow, flow: &FlowInner) {
    match &flow.stage {
        Stage::Welcome => ui.set_screen(0),
        Stage::WordCount { .. } => {
            // Deliberately does NOT clear `word-entry-error` here (unlike,
            // say, `SetPinFirst`'s unconditional `pin-error` reset below)
            // -- `PendingValidateRecovery`'s failure path sets that
            // message and transitions straight to this stage, and it
            // needs to survive this render to actually be seen. Cleared
            // explicitly instead, at the point a *fresh* Welcome ->
            // WordCount transition happens (`on_create_pressed`/
            // `on_recover_pressed`), where a stale message would
            // otherwise wrongly persist.
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

/// Performs the real work for whichever `Stage::Pending*` is currently
/// set, returning the stage that follows it. Only ever called right after
/// that pending stage's "Working…" frame has actually been rendered (see
/// the module doc comment) -- a no-op passthrough for every other stage.
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
            // Numbered here (not in app.slint) so the Mnemonic screen's
            // display grid only ever needs plain array indexing, never
            // int/string concatenation -- see that screen's own comment.
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
                // Only possible failure mode: the checksum doesn't match
                // (every individual word was already a real wordlist
                // entry -- the adaptive keyboard can't produce anything
                // else). No way to say *which* word was wrong from a
                // checksum alone, so recovery restarts from the count
                // choice rather than trying to salvage a partial retry.
                log::warn!("ui: recovery phrase failed validation: {e}");
                ui.set_word_entry_error("Recovery phrase invalid, try again".into());
                Stage::WordCount { purpose: Purpose::Recover }
            }
        },
        other => other,
    }
}

/// Handles one PIN digit (0-9) reaching `PIN_LENGTH` -- fast, non-crypto
/// transitions only (matching first-entry vs confirm, or handing off to a
/// `Stage::Pending*` for the main loop to actually execute). Called from
/// the `pin-digit-pressed` callback once the buffer is full.
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
        other => other, // Welcome/WordCount/Mnemonic/WordEntry/Done/Wiped: digits ignored (not the PIN screen)
    };
}

/// Blocks until either a real seed is ready (returns it, along with the
/// `AppWindow` this function built -- callers reuse this same instance
/// afterward, e.g. to show the Home/pairing/sign-confirm/factory-reset
/// screens, rather than constructing a second, ambiguous one against the
/// same underlying `MinimalSoftwareWindow`) or the device gets wiped
/// (never returns -- see `Stage::Wiped`'s doc comment).
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

        // Checked right after the render above, so a Home screen set by
        // the *previous* iteration's pending-work step (below) has
        // actually been drawn at least once before returning -- returning
        // any earlier would skip straight past it, same class of bug as
        // the "Working…" screen this whole split exists to avoid.
        if let Stage::Done(seed) = &flow.borrow().stage {
            return Ok((ui, *seed));
        }

        // Only after a render (possibly a freshly-set "Working…" screen)
        // has actually been drawn: run any pending slow crypto call.
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
