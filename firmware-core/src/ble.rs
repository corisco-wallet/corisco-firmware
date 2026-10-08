//! BLE GATT signing service -- the transport between the phone app and the
//! device's signing logic. A binary (`postcard`) request/response protocol
//! (commit/sign/identity-key ops) since BLE payloads are precious.
//!
//! Built on the `esp32-nimble` crate (NimBLE host), not `esp-idf-svc`'s
//! `bt::ble::gatt` module (Bluedroid) -- Bluedroid's advertising was
//! deterministically rejected by the controller on this chip/IDF version
//! ("Cmd Disallowed" on every `start_advertising()` HCI call). NimBLE is
//! also ESP-IDF's own recommended host stack for BLE-only chips like the S3.
//!
//! ## Pairing
//!
//! Real BLE bonding, not an open connection: `SecurityIOCap::DisplayOnly`
//! (this device has a screen, no keyboard) -- the phone's OS shows "Enter
//! the PIN shown on [device]", and `on_passkey_request` generates a fresh
//! random 6-digit passkey per pairing attempt (not a fixed one) and shows
//! it via `passkey_tx`. `CONFIG_BT_NIMBLE_NVS_PERSIST=y` (sdkconfig.defaults)
//! keeps the bond across reboots, so this only happens once per phone.
//!
//! ## Protocol
//!
//! One service, two characteristics:
//! - **Request** (write, encrypted+authenticated): phone -> device. A
//!   `Request` (see below), postcard-encoded, then split into frames (see
//!   "Framing").
//! - **Response** (indicate): device -> phone. A `Response`, framed the
//!   same way.
//!
//! Only one request may be in flight at a time -- matches the HSM/hardware-
//! wallet model (no concurrent signing operations), and keeps reassembly
//! trivial (no request-id multiplexing needed at the framing layer).
//!
//! ## Framing
//!
//! BLE's negotiated ATT MTU (default 23, phones commonly negotiate up to
//! ~185-247) caps how much fits in one write/indication. Rather than assume
//! a large MTU gets negotiated, every message (request or response) is
//! split into frames:
//! - First frame: `[len_lo, len_hi, payload...]` -- `len` is the total
//!   postcard-encoded message length (u16 LE, so up to 64KB -- generous for
//!   what these messages ever need).
//! - Continuation frames: raw payload bytes, no header.
//!
//! The receiver buffers frames until it has collected `len` bytes, then
//! postcard-decodes. Indications (not notifications) are used for responses
//! specifically because they're confirmed at the link layer -- sending the
//! next frame only after the previous one is confirmed gives us in-order,
//! no-loss delivery for free, without a bespoke ACK scheme on top.
//! `on_notify_tx` is esp32-nimble's confirmation signal for this.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use esp32_nimble::utilities::mutex::Mutex as NimbleMutex;
use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{enums::*, BLEAdvertisementData, BLECharacteristic, BLEDevice, NimbleProperties};
use frost_secp256k1_tr::Identifier;
use log::info;
use corisco_crypto_core::frost::{NonceCommitment, SigningCommitments, SigningNonces};
use corisco_crypto_core::SparkKeyRoots;

pub use corisco_protocol::{REQUEST_CHARACTERISTIC_UUID, RESPONSE_CHARACTERISTIC_UUID, SERVICE_UUID};
use corisco_protocol::{KeyDerivationRef, Request, Response, ShareWire};

use crate::pending::{AuthorizedLeaves, PendingStore};

const DEFAULT_ATT_MTU: u16 = 23;
const ATT_OVERHEAD: u16 = 3;

fn uuid(id: u128) -> BleUuid {
    BleUuid::Uuid128(id.to_le_bytes())
}

// ---------------------------------------------------------------------------
// Wire protocol
// ---------------------------------------------------------------------------

fn resolve_private_key(roots: &SparkKeyRoots, d: &KeyDerivationRef) -> Result<[u8; 32], String> {
    match d {
        KeyDerivationRef::Leaf { leaf_id } => roots.derive_leaf_key(leaf_id).map_err(|e| e.to_string()),
        KeyDerivationRef::Deposit => Ok(roots.deposit.private_key),
        KeyDerivationRef::StaticDeposit { idx } => {
            roots.static_deposit_private_key(*idx).map_err(|e| e.to_string())
        }
        KeyDerivationRef::Ecies { ciphertext } => {
            let plaintext = corisco_crypto_core::decrypt_ecies(ciphertext, &roots.identity.private_key)?;
            plaintext
                .try_into()
                .map_err(|v: Vec<u8>| format!("decrypted ECIES payload was {} bytes, expected 32", v.len()))
        }
        KeyDerivationRef::Random => Ok(corisco_crypto_core::random_private_key()),
    }
}

/// `ShareWire` lives in `corisco-protocol`, so this can't be a `From` impl
/// (orphan rule).
fn share_wire(vs: corisco_crypto_core::vss::VerifiableSecretShare) -> ShareWire {
    ShareWire {
        threshold: vs.share.threshold as u32,
        index: vs.share.index,
        share: vs.share.share.to_vec(),
        proofs: vs.proofs.iter().map(|p| p.to_vec()).collect(),
    }
}

fn err(message: impl std::fmt::Display) -> Response {
    Response::Error { message: message.to_string() }
}

/// Identity signatures authorize logins and transfers alike, and the device only sees a digest, so each one
/// needs a tap unless the user turned confirmation off on the device.
fn request_identity_signature(
    signer: &Arc<Signer>,
    responder: ResponseChannel,
    message: Vec<u8>,
    scheme: IdentityScheme,
) -> Option<Response> {
    let request = IdentityConfirmationRequest { signer: signer.clone(), message, scheme, responder };
    if signer.require_confirmation.load(Ordering::Relaxed) {
        if signer.confirm_tx.send(Confirmation::Identity(Box::new(request))).is_err() {
            return Some(err("no confirmation receiver (UI thread gone?)"));
        }
    } else if signer.work_tx.send(DeferredRequest::SignIdentity { request }).is_err() {
        return Some(err("no deferred-work receiver (main thread gone?)"));
    }
    None
}

/// Returns `None` for `Commit`, `Sign`, `GetLeafPublicKey`,
/// `SignSchnorrIdentity`, and `SignEcdsaIdentity` -- every request that
/// does *fresh* elliptic-curve computation (BIP32 derivation, FROST nonce
/// generation, Schnorr/ECDSA signing) defers that work off this call stack
/// (see `DeferredRequest`/`run_deferred`, `complete_sign`). This isn't
/// optional: this function runs synchronously inside `on_write`, on the
/// NimBLE host task's own (deliberately small) stack, which overflows
/// partway through this kind of math if it runs inline. Bumping
/// `CONFIG_BT_NIMBLE_HOST_TASK_STACK_SIZE` to accommodate it isn't viable
/// either -- total internal RAM is tight enough (~150KB free/~90KB largest
/// contiguous block right before BLE even starts, competing with the main
/// task's 128KB stack) that a large-enough bump starves NimBLE's own
/// internal buffers instead (`BLE_ERR_MEM_CAPACITY` on
/// `advertising.lock().start()`). `GetIdentityPublicKey`/
/// `GetDepositPublicKey` are the only pubkey-shaped requests answered
/// inline -- they just return bytes already computed once at boot
/// (`SparkKeyRoots`), no fresh math at all.
fn handle_request(signer: &Arc<Signer>, responder: ResponseChannel, req: Request) -> Option<Response> {
    let roots = &signer.roots;
    let defer = |work: DeferredRequest| -> Option<Response> {
        if signer.work_tx.send(work).is_err() {
            return Some(err("no deferred-work receiver (main thread gone?)"));
        }
        None
    };
    Some(match req {
        Request::Commit => return defer(DeferredRequest::Commit { signer: signer.clone(), responder }),
        Request::GetLeafPublicKey { leaf_id } => {
            return defer(DeferredRequest::GetLeafPublicKey { signer: signer.clone(), leaf_id, responder })
        }
        Request::SignSchnorrIdentity { message } => {
            return request_identity_signature(signer, responder, message, IdentityScheme::Schnorr)
        }
        Request::SignEcdsaIdentity { message, compact } => {
            return request_identity_signature(signer, responder, message, IdentityScheme::Ecdsa { compact })
        }
        Request::Sign {
            commitment_id,
            leaf_id,
            message,
            statechain_commitments,
            verifying_key,
            adaptor_public_key,
            requires_confirmation,
            amount_sats,
            destination,
        } => {
            let Some((nonce, self_commitment)) = signer.pending_commitments.lock().unwrap().take(commitment_id)
            else {
                return Some(err(format!("unknown or already-used commitment_id: {commitment_id}")));
            };

            let leaf_key = match roots.derive_leaf_key(&leaf_id) {
                Ok(k) => k,
                Err(e) => return Some(err(format!("{e:?}"))),
            };

            let mut statechain_commitment_map: BTreeMap<Identifier, SigningCommitments> = BTreeMap::new();
            for sc in statechain_commitments {
                let id = match Identifier::deserialize(&sc.identifier) {
                    Ok(id) => id,
                    Err(e) => return Some(err(format!("{e:?}"))),
                };
                let hiding = match NonceCommitment::deserialize(&sc.hiding) {
                    Ok(h) => h,
                    Err(e) => return Some(err(format!("{e:?}"))),
                };
                let binding = match NonceCommitment::deserialize(&sc.binding) {
                    Ok(b) => b,
                    Err(e) => return Some(err(format!("{e:?}"))),
                };
                statechain_commitment_map.insert(id, SigningCommitments::new(hiding, binding));
            }

            // Boxed, not sent by value: `std::sync::mpmc`'s channel
            // allocates each internal storage block sized for ~31 slots of
            // the element type at once, and `SignConfirmationRequest`
            // (full FROST nonce/commitment structs, not just ids) is large
            // enough that one such block is itself a ~20.7KB allocation --
            // a real risk on a chip this tight on contiguous internal RAM
            // (a live BLE connection's own GATT/security/MTU overhead
            // alone costs ~90-100KB). Boxing shrinks each channel slot to
            // a pointer, cutting that one-time block allocation by roughly
            // the same factor as `SignConfirmationRequest`'s own size.
            let confirm_req = Box::new(SignConfirmationRequest {
                leaf_id,
                message,
                leaf_key,
                nonce,
                self_commitment,
                statechain_commitments: statechain_commitment_map,
                verifying_key,
                adaptor_public_key,
                amount_sats,
                destination,
                responder,
            });
            // Default deny: only a leaf this device saw being claimed may skip the prompt, and the
            // phone's flag can only add a prompt, never remove one. The Settings toggle still
            // overrides everything when the user has turned confirmation off on-device.
            let claim_authorized = signer.claimed_leaves.lock().unwrap().is_authorized(&confirm_req.leaf_id);
            let needs_prompt = requires_confirmation || !claim_authorized;
            if needs_prompt && signer.require_confirmation.load(Ordering::Relaxed) {
                if signer.confirm_tx.send(Confirmation::Sign(confirm_req)).is_err() {
                    return Some(err("no confirmation receiver (UI thread gone?)"));
                }
            } else if signer.work_tx.send(DeferredRequest::AutoSign { req: confirm_req }).is_err() {
                return Some(err("no deferred-work receiver (main thread gone?)"));
            }
            return None;
        }
        Request::GetIdentityPublicKey => {
            Response::PublicKey { public_key: roots.identity.public_key_compressed().to_vec() }
        }
        Request::GetDepositPublicKey => {
            Response::PublicKey { public_key: roots.deposit.public_key_compressed().to_vec() }
        }
        Request::SubtractAndSplitSecretWithProofs { first, second, threshold, num_shares } => {
            return defer(DeferredRequest::SubtractAndSplitSecretWithProofs {
                signer: signer.clone(),
                first,
                second,
                threshold,
                num_shares,
                responder,
            })
        }
        Request::DecryptEciesToPublicKey { ciphertext } => {
            return defer(DeferredRequest::DecryptEciesToPublicKey { signer: signer.clone(), ciphertext, responder })
        }
        Request::SubtractSplitAndEncrypt { first, second, receiver_public_key, threshold, num_shares } => {
            return defer(DeferredRequest::SubtractSplitAndEncrypt {
                signer: signer.clone(),
                first,
                second,
                receiver_public_key,
                threshold,
                num_shares,
                responder,
            })
        }
    })
}

// ---------------------------------------------------------------------------
// GATT server plumbing (esp32-nimble)
// ---------------------------------------------------------------------------

/// Buffers frames arriving on the Request characteristic until a full
/// message has been collected (see module doc "Framing").
#[derive(Default)]
struct RequestReassembly {
    expected_len: Option<usize>,
    buf: Vec<u8>,
}

struct Signer {
    roots: SparkKeyRoots,
    pending_commitments: Mutex<PendingStore<(SigningNonces, SigningCommitments)>>,
    next_commitment_id: AtomicU32,
    claimed_leaves: Mutex<AuthorizedLeaves>,
    confirm_tx: mpsc::Sender<Confirmation>,
    work_tx: mpsc::Sender<DeferredRequest>,
    /// Device-level override of the wire protocol's own
    /// `requires_confirmation` flag -- the Settings screen's "require
    /// confirmation" toggle (`main.rs`'s wiring), shared so flipping it
    /// takes effect on the very next `Sign` with no reboot needed. `false`
    /// means every `Sign` auto-signs regardless of what the app requested
    /// -- see `handle_request`'s `Request::Sign` arm, the only place this
    /// is read. Seeded at boot from `storage::Storage::get_require_confirmation`
    /// (defaults to `true`, the safe choice, if never explicitly set).
    require_confirmation: Arc<AtomicBool>,
}

/// Tracks whether the most recently sent indication has been confirmed
/// yet -- `send_response_frames` blocks on this (via the paired
/// `Condvar`) so it only sends the next frame once the previous one is
/// acknowledged. Signaled from `on_notify_tx`, which esp32-nimble calls
/// once a notify/indicate attempt completes (success or failure).
#[derive(Default)]
struct IndicateState {
    awaiting_confirm: bool,
}

/// Everything needed to send a response to a specific connection later,
/// after a `Sign` request has been deferred to the UI thread for
/// confirmation -- the same pieces `on_request_frame` already had in
/// scope for an immediate response, just bundled so they can travel
/// inside a `SignConfirmationRequest` instead.
#[derive(Clone)]
struct ResponseChannel {
    response_characteristic: Arc<NimbleMutex<BLECharacteristic>>,
    indicate_state: Arc<Mutex<IndicateState>>,
    indicate_cv: Arc<Condvar>,
    conn_handle: u16,
}

impl ResponseChannel {
    fn send(&self, response: &Response) {
        match postcard::to_allocvec(response) {
            Ok(encoded) => send_response_frames(
                &self.response_characteristic,
                &self.indicate_state,
                &self.indicate_cv,
                self.conn_handle,
                &encoded,
            ),
            Err(e) => log::warn!("ble: failed to encode response: {e:?}"),
        }
    }
}

/// A `Sign` request waiting on the on-screen confirmation gate. `leaf_id`/
/// `message` are the fallback the UI shows when the app didn't supply
/// `amount_sats`/`destination` (see `Request::Sign`'s doc comment on why
/// those are app-asserted, not verified); everything else is exactly what
/// `frost_sign` needs, already validated and looked up by `handle_request`
/// before deferring here, so `complete_sign` can just run it directly on
/// accept.
pub struct SignConfirmationRequest {
    pub leaf_id: String,
    pub message: Vec<u8>,
    leaf_key: [u8; 32],
    nonce: SigningNonces,
    self_commitment: SigningCommitments,
    statechain_commitments: BTreeMap<Identifier, SigningCommitments>,
    verifying_key: Vec<u8>,
    adaptor_public_key: Option<Vec<u8>>,
    pub amount_sats: Option<u64>,
    pub destination: Option<String>,
    responder: ResponseChannel,
}

/// Runs the real signature (on accept) or sends an `Error` response (on
/// decline), completing a request `handle_request` deferred earlier.
/// Called from the UI thread once the user taps Accept/Decline on the
/// confirmation screen.
pub fn complete_sign(req: Box<SignConfirmationRequest>, accept: bool) {
    let response = if !accept {
        err("declined on device")
    } else {
        match corisco_crypto_core::frost::frost_sign(
            &req.message,
            &req.leaf_key,
            &req.nonce,
            req.self_commitment,
            req.statechain_commitments,
            &req.verifying_key,
            req.adaptor_public_key.as_deref(),
        ) {
            Ok(share_bytes) => Response::Sign { signature_share: share_bytes },
            Err(e) => err(format!("{e:?}")),
        }
    };
    req.responder.send(&response);
}

#[derive(Clone, Copy)]
pub enum IdentityScheme {
    Schnorr,
    Ecdsa { compact: bool },
}

/// An identity-key signature waiting on the on-screen confirmation gate.
pub struct IdentityConfirmationRequest {
    pub message: Vec<u8>,
    scheme: IdentityScheme,
    signer: Arc<Signer>,
    responder: ResponseChannel,
}

/// What the confirm screen is asked to show.
pub enum Confirmation {
    Sign(Box<SignConfirmationRequest>),
    Identity(Box<IdentityConfirmationRequest>),
}

/// Called from the UI thread once the user taps Accept/Decline.
pub fn complete_confirmation(confirmation: Confirmation, accept: bool) {
    match confirmation {
        Confirmation::Sign(req) => complete_sign(req, accept),
        Confirmation::Identity(req) if accept => complete_identity(*req),
        Confirmation::Identity(req) => req.responder.send(&err("declined on device")),
    }
}

fn complete_identity(request: IdentityConfirmationRequest) {
    let key = &request.signer.roots.identity.private_key;
    let response = match request.scheme {
        IdentityScheme::Schnorr => {
            let sig = corisco_crypto_core::sign_schnorr(key, &request.message);
            Response::Signature { signature: sig.to_bytes().to_vec() }
        }
        // DER by default: spark_authn's `verify_challenge` rejects compact r||s.
        IdentityScheme::Ecdsa { compact } => match <[u8; 32]>::try_from(request.message.as_slice()) {
            Ok(digest) => {
                let sig = corisco_crypto_core::sign_ecdsa_prehashed(key, &digest);
                let signature = if compact { sig.to_bytes().to_vec() } else { sig.to_der().to_bytes().to_vec() };
                Response::Signature { signature }
            }
            Err(_) => err("message must be exactly 32 bytes (a digest)"),
        },
    };
    request.responder.send(&response);
}

/// A request waiting on fresh elliptic-curve computation -- deferred off
/// `on_write`'s stack for the reason `handle_request`'s doc comment
/// explains, just without any user-facing gate (unlike `Sign`): `run_deferred`
/// runs it immediately, no accept/decline involved. Carries the whole
/// `Arc<Signer>` (not just `roots`) since `Commit` also needs to register
/// the resulting nonce into `pending_commitments` for the eventual
/// matching `Sign`.
pub enum DeferredRequest {
    Commit { signer: Arc<Signer>, responder: ResponseChannel },
    GetLeafPublicKey { signer: Arc<Signer>, leaf_id: String, responder: ResponseChannel },
    SignIdentity { request: IdentityConfirmationRequest },
    SubtractAndSplitSecretWithProofs {
        signer: Arc<Signer>,
        first: KeyDerivationRef,
        second: KeyDerivationRef,
        threshold: u32,
        num_shares: u32,
        responder: ResponseChannel,
    },
    DecryptEciesToPublicKey { signer: Arc<Signer>, ciphertext: Vec<u8>, responder: ResponseChannel },
    /// A `Sign` request with `requires_confirmation: false` -- same
    /// `frost_sign` computation `complete_sign` runs on Accept, just
    /// triggered automatically instead of waiting on a screen tap. Reuses
    /// `SignConfirmationRequest` as-is (no on-screen use of its
    /// `leaf_id`/`message` fields here) rather than a separate struct.
    AutoSign { req: Box<SignConfirmationRequest> },
    SubtractSplitAndEncrypt {
        signer: Arc<Signer>,
        first: KeyDerivationRef,
        second: KeyDerivationRef,
        receiver_public_key: Vec<u8>,
        threshold: u32,
        num_shares: u32,
        responder: ResponseChannel,
    },
}

/// Runs the real computation and sends the response (or an `Error` on
/// failure). Called from the main/UI thread's render loop, which has a
/// proven-sufficient stack for this class of curve math (it's the same
/// thread `frost_self_test` already runs on at boot) -- see
/// `handle_request`'s doc comment for why this can't run inline in
/// `on_write` instead.
pub fn run_deferred(req: DeferredRequest) {
    match req {
        DeferredRequest::Commit { signer, responder } => {
            let response = match corisco_crypto_core::frost::frost_commit(&signer.roots.identity.private_key) {
                Ok((nonce, commitment)) => {
                    match (commitment.hiding().serialize(), commitment.binding().serialize()) {
                        (Ok(hiding), Ok(binding)) => {
                            let commitment_id = signer.next_commitment_id.fetch_add(1, Ordering::Relaxed);
                            signer.pending_commitments.lock().unwrap().insert(commitment_id, (nonce, commitment));
                            Response::Commit { commitment_id, hiding, binding }
                        }
                        (Err(e), _) | (_, Err(e)) => err(format!("{e:?}")),
                    }
                }
                Err(e) => err(format!("{e:?}")),
            };
            responder.send(&response);
        }
        DeferredRequest::GetLeafPublicKey { signer, leaf_id, responder } => {
            let response = match signer.roots.derive_leaf_public_key(&leaf_id) {
                Ok(pk) => Response::PublicKey { public_key: pk.to_vec() },
                Err(e) => err(format!("{e:?}")),
            };
            responder.send(&response);
        }
        DeferredRequest::SignIdentity { request } => complete_identity(request),
        DeferredRequest::SubtractAndSplitSecretWithProofs { signer, first, second, threshold, num_shares, responder } => {
            let Some(claimed_leaf_id) = KeyDerivationRef::claim_leaf_id(&first, &second).map(str::to_owned) else {
                responder.send(&err("SubtractAndSplitSecretWithProofs only tweaks an incoming key onto a leaf"));
                return;
            };
            let response = match (resolve_private_key(&signer.roots, &first), resolve_private_key(&signer.roots, &second)) {
                (Ok(a), Ok(b)) => {
                    let diff = corisco_crypto_core::subtract_private_keys(&a, &b);
                    match corisco_crypto_core::vss::split_secret_with_proofs(&diff, threshold as usize, num_shares as usize) {
                        Ok(shares) => {
                            signer.claimed_leaves.lock().unwrap().authorize(claimed_leaf_id);
                            Response::Shares { shares: shares.into_iter().map(share_wire).collect() }
                        }
                        Err(e) => err(e),
                    }
                }
                (Err(e), _) | (_, Err(e)) => err(e),
            };
            responder.send(&response);
        }
        DeferredRequest::DecryptEciesToPublicKey { signer, ciphertext, responder } => {
            let response = match corisco_crypto_core::decrypt_ecies(&ciphertext, &signer.roots.identity.private_key) {
                Ok(plaintext) => match <[u8; 32]>::try_from(plaintext.as_slice()) {
                    Ok(private_key) => {
                        let public_key = corisco_crypto_core::private_key_to_public_key_compressed(&private_key);
                        Response::PublicKey { public_key: public_key.to_vec() }
                    }
                    Err(_) => err(format!("decrypted ECIES payload was {} bytes, expected 32", plaintext.len())),
                },
                Err(e) => err(e),
            };
            responder.send(&response);
        }
        DeferredRequest::AutoSign { req } => complete_sign(req, true),
        DeferredRequest::SubtractSplitAndEncrypt {
            signer,
            first,
            second,
            receiver_public_key,
            threshold,
            num_shares,
            responder,
        } => {
            if !KeyDerivationRef::is_leaf_swap(&first, &second) {
                responder.send(&err("SubtractSplitAndEncrypt only swaps a leaf key for a random one"));
                return;
            }
            let response = match (resolve_private_key(&signer.roots, &first), resolve_private_key(&signer.roots, &second)) {
                (Ok(a), Ok(b)) => {
                    let diff = corisco_crypto_core::subtract_private_keys(&a, &b);
                    match corisco_crypto_core::vss::split_secret_with_proofs(&diff, threshold as usize, num_shares as usize) {
                        Ok(shares) => match <[u8; 33]>::try_from(receiver_public_key.as_slice()) {
                            Ok(receiver_public_key) => match corisco_crypto_core::encrypt_ecies(&b, &receiver_public_key) {
                                Ok(secret_cipher) => Response::SubtractSplitAndEncrypt {
                                    shares: shares.into_iter().map(share_wire).collect(),
                                    secret_cipher,
                                },
                                Err(e) => err(e),
                            },
                            Err(_) => err(format!(
                                "receiver_public_key must be 33 bytes, got {}",
                                receiver_public_key.len()
                            )),
                        },
                        Err(e) => err(e),
                    }
                }
                (Err(e), _) | (_, Err(e)) => err(e),
            };
            responder.send(&response);
        }
    }
}

/// Sends `payload` to `conn_handle` as a sequence of confirmed
/// indications, chunked per the connection's negotiated MTU (see module
/// doc "Framing"). Blocks between frames until each is confirmed.
fn send_response_frames(
    characteristic: &Arc<NimbleMutex<BLECharacteristic>>,
    indicate_state: &Mutex<IndicateState>,
    indicate_cv: &Condvar,
    conn_handle: u16,
    payload: &[u8],
) {
    let mtu = unsafe { esp_idf_svc::sys::ble_att_mtu(conn_handle) };
    let mtu = if mtu == 0 { DEFAULT_ATT_MTU } else { mtu };
    let chunk_size = mtu.saturating_sub(ATT_OVERHEAD).max(1) as usize;

    let len = (payload.len() as u16).to_le_bytes();
    let mut frames: Vec<Vec<u8>> = Vec::new();

    let mut first = Vec::with_capacity(chunk_size);
    first.extend_from_slice(&len);
    let first_payload_room = chunk_size.saturating_sub(2);
    let (first_chunk, mut rest) = payload.split_at(payload.len().min(first_payload_room));
    first.extend_from_slice(first_chunk);
    frames.push(first);

    while !rest.is_empty() {
        let take = rest.len().min(chunk_size);
        let (chunk, remainder) = rest.split_at(take);
        frames.push(chunk.to_vec());
        rest = remainder;
    }

    for frame in frames {
        let mut state = indicate_state.lock().unwrap();
        while state.awaiting_confirm {
            state = indicate_cv.wait(state).unwrap();
        }
        {
            let mut c = characteristic.lock();
            c.set_value(&frame);
            state.awaiting_confirm = true;
            if let Err(e) = c.notify_with(&frame, conn_handle) {
                log::warn!("ble: indicate failed: {e:?}");
                state.awaiting_confirm = false;
            }
        }
    }
}

/// Feeds one frame into the reassembly buffer; once a full message has
/// arrived, decodes + dispatches it. Runs synchronously on the NimBLE host
/// task's callback -- fine even for `Commit`/`Sign` now, since
/// `handle_request` never runs their actual curve math itself: it hands
/// off to `signer.commit_tx`/`signer.confirm_tx` and returns `None`
/// immediately, and `complete_commit`/`complete_sign` (called later, from
/// the main/UI thread) are what actually compute and send the response.
#[allow(clippy::too_many_arguments)]
fn on_request_frame(
    signer: &Arc<Signer>,
    reassembly: &Mutex<RequestReassembly>,
    response_characteristic: &Arc<NimbleMutex<BLECharacteristic>>,
    indicate_state: &Arc<Mutex<IndicateState>>,
    indicate_cv: &Arc<Condvar>,
    conn_handle: u16,
    frame: &[u8],
) {
    let complete_message = {
        let mut reassembly = reassembly.lock().unwrap();
        match reassembly.expected_len {
            None => {
                if frame.len() < 2 {
                    log::warn!("ble: first request frame too short ({} bytes)", frame.len());
                    return;
                }
                let len = u16::from_le_bytes([frame[0], frame[1]]) as usize;
                reassembly.expected_len = Some(len);
                reassembly.buf.clear();
                reassembly.buf.extend_from_slice(&frame[2..]);
            }
            Some(_) => {
                reassembly.buf.extend_from_slice(frame);
            }
        }

        let expected = reassembly.expected_len.unwrap();
        if reassembly.buf.len() < expected {
            None
        } else {
            let msg = reassembly.buf[..expected].to_vec();
            reassembly.expected_len = None;
            reassembly.buf.clear();
            Some(msg)
        }
    };

    let Some(msg) = complete_message else { return };

    let responder = ResponseChannel {
        response_characteristic: response_characteristic.clone(),
        indicate_state: indicate_state.clone(),
        indicate_cv: indicate_cv.clone(),
        conn_handle,
    };

    let response = match postcard::from_bytes::<Request>(&msg) {
        Ok(req) => handle_request(signer, responder.clone(), req),
        Err(e) => Some(err(format!("bad request framing/encoding: {e:?}"))),
    };

    // `None` means `handle_request` deferred this to `signer.confirm_tx`
    // (a `Sign` request awaiting on-screen confirmation) -- the response
    // will be sent later, by `complete_sign`, not here.
    if let Some(response) = response {
        responder.send(&response);
    }
}

/// A passkey shown to the user during pairing -- sent to the UI thread so
/// it can be displayed on-device (`SecurityIOCap::DisplayOnly` means the
/// phone can't show it; it must come from us).
pub struct PasskeyDisplay {
    pub passkey: u32,
}

/// Whether the radio is currently advertising (discoverable, no phone
/// connected yet) or has a phone connected -- sent to the UI thread so the
/// Home screen can show it instead of leaving BLE state entirely invisible
/// once past the pairing screen. Only two states: a peripheral can't
/// advertise while connected (see `on_disconnect`'s doc comment below), so
/// there's no third "idle/off" state to represent -- the radio is always
/// doing one or the other once `run` reaches `advertising.lock().start()`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BleStatus {
    Advertising,
    Connected,
}

/// Brings up the BLE GATT signing service and blocks forever, servicing
/// requests as they arrive. `passkey_tx` carries each freshly generated
/// pairing passkey to the UI thread for display; failing to send (e.g. no
/// receiver yet) is not fatal -- the phone's own pairing dialog still
/// shows *something* even if the device-side screen missed it. `confirm_tx`
/// carries `Sign` requests to the UI thread's confirmation screen -- see
/// `handle_request`/`complete_sign`. `status_tx` carries advertising/
/// connected transitions to the UI thread for the Home screen's BLE
/// indicator -- same "failing to send isn't fatal" treatment as
/// `passkey_tx`, since a missed status update just means a stale
/// indicator, not a functional problem. `pairing_done_tx` signals the UI
/// thread to leave the pairing-passkey screen once bonding actually
/// finishes -- nothing else tells it to, so without this it would stay
/// there forever after a successful pair unless something unrelated (a
/// sign request) happened to change the screen afterward.
pub fn run(
    roots: SparkKeyRoots,
    passkey_tx: mpsc::Sender<PasskeyDisplay>,
    confirm_tx: mpsc::Sender<Confirmation>,
    work_tx: mpsc::Sender<DeferredRequest>,
    status_tx: mpsc::Sender<BleStatus>,
    require_confirmation: Arc<AtomicBool>,
    pairing_done_tx: mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let device = BLEDevice::take();

    // DisplayOnly: this device has a screen, no keyboard -- the phone's OS
    // shows "Enter the PIN shown on <device>", we generate + display a
    // fresh 6-digit passkey per pairing attempt (`on_passkey_request`
    // below), not a single fixed one. `resolve_rpa` handles iOS's private
    // address rotation for a *bonded* device to keep reconnecting to it.
    device.security().set_auth(AuthReq::all()).set_io_cap(SecurityIOCap::DisplayOnly).resolve_rpa();

    let signer = Arc::new(Signer {
        roots,
        pending_commitments: Mutex::new(PendingStore::new()),
        next_commitment_id: AtomicU32::new(0),
        claimed_leaves: Mutex::new(AuthorizedLeaves::default()),
        confirm_tx,
        work_tx,
        require_confirmation,
    });

    let server = device.get_server();

    {
        let passkey_tx = passkey_tx.clone();
        server.on_passkey_request(move || {
            let passkey = rng_passkey();
            if passkey_tx.send(PasskeyDisplay { passkey }).is_err() {
                log::warn!("ble: no receiver for passkey display (UI thread gone?)");
            }
            info!("ble: pairing passkey {passkey:06}");
            passkey
        });
    }
    server.on_authentication_complete(move |_, desc, result| {
        info!("ble: pairing complete for {desc:?}: {result:?}");
        if pairing_done_tx.send(()).is_err() {
            log::warn!("ble: no receiver for pairing-done signal (UI thread gone?)");
        }
    });
    {
        let status_tx = status_tx.clone();
        server.on_connect(move |_server, desc| {
            info!("ble: connected {desc:?}");
            if status_tx.send(BleStatus::Connected).is_err() {
                log::warn!("ble: no receiver for status display (UI thread gone?)");
            }
        });
    }
    // A peripheral can't advertise while connected -- NimBLE stops
    // advertising the moment a central connects, and never resumes on its
    // own. Restarting it here covers every disconnect path (a clean
    // `disconnect()` from the app, the app dying without one, or the link
    // simply timing out) with one fix: without it, the device stays
    // connected from its own perspective after any non-clean disconnect,
    // with no code path left that would ever call
    // `advertising.lock().start()` again short of a power cycle.
    let advertising = device.get_advertising();
    {
        let status_tx = status_tx.clone();
        let signer = signer.clone();
        server.on_disconnect(move |desc, reason| {
            info!("ble: disconnected {desc:?}: {reason:?}");
            signer.pending_commitments.lock().unwrap().clear();
            if let Err(e) = advertising.lock().start() {
                log::error!("ble: failed to restart advertising after disconnect: {e:?}");
            }
            if status_tx.send(BleStatus::Advertising).is_err() {
                log::warn!("ble: no receiver for status display (UI thread gone?)");
            }
        });
    }

    let service = server.create_service(uuid(SERVICE_UUID));

    let response_characteristic =
        service.lock().create_characteristic(uuid(RESPONSE_CHARACTERISTIC_UUID), NimbleProperties::INDICATE);

    let indicate_state = Arc::new(Mutex::new(IndicateState::default()));
    let indicate_cv = Arc::new(Condvar::new());
    {
        let indicate_state = indicate_state.clone();
        let indicate_cv = indicate_cv.clone();
        response_characteristic.lock().on_notify_tx(move |tx| {
            log::info!("ble: indicate {:?}", tx.status());
            let mut state = indicate_state.lock().unwrap();
            state.awaiting_confirm = false;
            indicate_cv.notify_all();
        });
    }

    let reassembly = Arc::new(Mutex::new(RequestReassembly::default()));
    {
        let signer = signer.clone();
        let response_characteristic = response_characteristic.clone();
        let indicate_state = indicate_state.clone();
        let indicate_cv = indicate_cv.clone();
        let request_characteristic = service.lock().create_characteristic(
            uuid(REQUEST_CHARACTERISTIC_UUID),
            NimbleProperties::WRITE | NimbleProperties::WRITE_ENC | NimbleProperties::WRITE_AUTHEN,
        );
        request_characteristic.lock().on_write(move |args| {
            let conn_handle = args.desc().conn_handle();
            on_request_frame(&signer, &reassembly, &response_characteristic, &indicate_state, &indicate_cv, conn_handle, args.recv_data());
        });
    }

    // Same `advertising` handle `on_disconnect` above captured -- `&'static
    // Mutex<_>` is `Copy`, so that `move` closure only copied the
    // reference, this binding is still valid.
    BLEDevice::set_device_name(&device_name())?;
    advertising.lock().set_data(BLEAdvertisementData::new().name(&device_name()).add_service_uuid(uuid(SERVICE_UUID)))?;
    advertising.lock().start()?;
    if status_tx.send(BleStatus::Advertising).is_err() {
        log::warn!("ble: no receiver for status display (UI thread gone?)");
    }

    info!("ble: identity pubkey {}", hex::encode(signer.roots.identity.public_key_compressed()));
    info!("ble: advertising started");

    loop {
        esp_idf_svc::hal::delay::FreeRtos::delay_ms(1000);
    }
}

/// Last 3 bytes of the BT MAC, uppercase hex: unique per chip and stable across wipes.
pub fn device_id() -> String {
    let mut mac = [0u8; 6];
    unsafe { esp_idf_svc::sys::esp_read_mac(mac.as_mut_ptr(), esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_BT) };
    format!("{:02X}{:02X}{:02X}", mac[3], mac[4], mac[5])
}

/// Advertised BLE name; the app's `isSignerName` matches on the `Corisco-` prefix.
pub fn device_name() -> String {
    format!("Corisco-{}", device_id())
}

/// A fresh random 6-digit (000000..=999999) passkey, via the same
/// hardware-TRNG source as everything else that needs real randomness on
/// this device (see `rng.rs`).
fn rng_passkey() -> u32 {
    let mut buf = [0u8; 4];
    unsafe { esp_idf_svc::sys::esp_fill_random(buf.as_mut_ptr().cast(), buf.len()) };
    u32::from_le_bytes(buf) % 1_000_000
}
