//! BLE GATT signing service between the phone app and the device's keys.
//!
//! Uses NimBLE (`esp32-nimble`), not Bluedroid: the controller rejects Bluedroid's advertising on
//! this chip/IDF version ("Cmd Disallowed").
//!
//! Framing: one request in flight; the first frame is `[len_lo, len_hi, payload...]` and the rest
//! are raw payload, so nothing depends on a large MTU. Responses are indications, each sent only
//! after the previous one is confirmed, which gives in-order delivery without a custom ACK.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use corisco_crypto_core::frost::{NonceCommitment, SigningCommitments, SigningNonces};
use corisco_crypto_core::SparkKeyRoots;
use esp32_nimble::utilities::mutex::Mutex as NimbleMutex;
use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{enums::*, BLEAdvertisementData, BLECharacteristic, BLEDevice, NimbleProperties};
use frost_secp256k1_tr::Identifier;
use log::info;

use corisco_protocol::{KeyDerivationRef, Request, Response, ShareWire};
pub use corisco_protocol::{REQUEST_CHARACTERISTIC_UUID, RESPONSE_CHARACTERISTIC_UUID, SERVICE_UUID};

use crate::pending::{AuthorizedLeaves, PendingStore};

const DEFAULT_ATT_MTU: u16 = 23;
const ATT_OVERHEAD: u16 = 3;

fn uuid(id: u128) -> BleUuid {
    BleUuid::Uuid128(id.to_le_bytes())
}

fn resolve_private_key(roots: &SparkKeyRoots, d: &KeyDerivationRef) -> Result<[u8; 32], String> {
    match d {
        KeyDerivationRef::Leaf { leaf_id } => roots.derive_leaf_key(leaf_id).map_err(|e| e.to_string()),
        KeyDerivationRef::Deposit => Ok(roots.deposit.private_key),
        KeyDerivationRef::StaticDeposit { idx } => roots.static_deposit_private_key(*idx).map_err(|e| e.to_string()),
        KeyDerivationRef::Ecies { ciphertext } => {
            let plaintext = corisco_crypto_core::decrypt_ecies(ciphertext, &roots.identity.private_key)?;
            plaintext
                .try_into()
                .map_err(|v: Vec<u8>| format!("decrypted ECIES payload was {} bytes, expected 32", v.len()))
        }
        KeyDerivationRef::Random => Ok(corisco_crypto_core::random_private_key()),
    }
}

/// `ShareWire` lives in `corisco-protocol`, so this can't be a `From` impl (orphan rule).
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

/// Returns `None` when the work is deferred: fresh EC math overflows the NimBLE host task's stack,
/// and RAM is too tight to grow that stack without starving NimBLE's own buffers.
fn handle_request(signer: &Arc<Signer>, responder: ResponseChannel, req: Request) -> Option<Response> {
    let roots = &signer.roots;
    let defer = |work: Work| -> Option<Response> {
        if signer.work_tx.send(DeferredRequest(work)).is_err() {
            return Some(err("no deferred-work receiver (main thread gone?)"));
        }
        None
    };
    Some(match req {
        Request::Commit => return defer(Work::Commit { signer: signer.clone(), responder }),
        Request::GetLeafPublicKey { leaf_id } => {
            return defer(Work::GetLeafPublicKey { signer: signer.clone(), leaf_id, responder })
        }
        Request::SignSchnorrIdentity { message } => {
            return defer(Work::SignSchnorrIdentity { signer: signer.clone(), message, responder })
        }
        Request::SignEcdsaIdentity { message, compact } => {
            return defer(Work::SignEcdsaIdentity { signer: signer.clone(), message, compact, responder })
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
            let Some((nonce, self_commitment)) = signer.pending_commitments.lock().unwrap().take(commitment_id) else {
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

            // Boxed: mpsc allocates ~31 slots per block, and by value this struct makes that a
            // ~20KB allocation on a chip short of contiguous internal RAM.
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
            // phone's flag can only add one. The on-device Settings toggle overrides both.
            let claim_authorized = signer.claimed_leaves.lock().unwrap().is_authorized(&confirm_req.leaf_id);
            let needs_prompt = requires_confirmation || !claim_authorized;
            if needs_prompt && signer.require_confirmation.load(Ordering::Relaxed) {
                if signer.confirm_tx.send(confirm_req).is_err() {
                    return Some(err("no confirmation receiver (UI thread gone?)"));
                }
            } else if signer.work_tx.send(DeferredRequest(Work::AutoSign { req: confirm_req })).is_err() {
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
            return defer(Work::SubtractAndSplitSecretWithProofs {
                signer: signer.clone(),
                first,
                second,
                threshold,
                num_shares,
                responder,
            })
        }
        Request::DecryptEciesToPublicKey { ciphertext } => {
            return defer(Work::DecryptEciesToPublicKey { signer: signer.clone(), ciphertext, responder })
        }
        Request::SubtractSplitAndEncrypt { first, second, receiver_public_key, threshold, num_shares } => {
            return defer(Work::SubtractSplitAndEncrypt {
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

#[derive(Default)]
struct RequestReassembly {
    expected_len: Option<usize>,
    buf: Vec<u8>,
}

// A nonce left in RAM after use is a key-leak risk; fail the build if a dependency bump drops wipe-on-drop.
const _: fn() = || {
    fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
    assert_zeroize_on_drop::<SigningNonces>();
};

struct Signer {
    roots: SparkKeyRoots,
    pending_commitments: Mutex<PendingStore<(SigningNonces, SigningCommitments)>>,
    next_commitment_id: AtomicU32,
    claimed_leaves: Mutex<AuthorizedLeaves>,
    confirm_tx: mpsc::Sender<Box<SignConfirmationRequest>>,
    work_tx: mpsc::Sender<DeferredRequest>,
    /// The on-device Settings toggle; `false` auto-signs every `Sign` regardless of the app's flag.
    require_confirmation: Arc<AtomicBool>,
}

/// Set while an indication is unconfirmed; `on_notify_tx` clears it so the next frame can go.
#[derive(Default)]
struct IndicateState {
    awaiting_confirm: bool,
}

/// Everything needed to answer a connection later, from another thread.
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

/// A validated `Sign` request waiting on the on-screen confirmation.
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

/// Signs on accept or answers with an error on decline; called from the UI thread.
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

/// Work moved off the NimBLE host task's stack (see `handle_request`).
pub struct DeferredRequest(Work);

enum Work {
    Commit {
        signer: Arc<Signer>,
        responder: ResponseChannel,
    },
    GetLeafPublicKey {
        signer: Arc<Signer>,
        leaf_id: String,
        responder: ResponseChannel,
    },
    SignSchnorrIdentity {
        signer: Arc<Signer>,
        message: Vec<u8>,
        responder: ResponseChannel,
    },
    SignEcdsaIdentity {
        signer: Arc<Signer>,
        message: Vec<u8>,
        compact: bool,
        responder: ResponseChannel,
    },
    SubtractAndSplitSecretWithProofs {
        signer: Arc<Signer>,
        first: KeyDerivationRef,
        second: KeyDerivationRef,
        threshold: u32,
        num_shares: u32,
        responder: ResponseChannel,
    },
    DecryptEciesToPublicKey {
        signer: Arc<Signer>,
        ciphertext: Vec<u8>,
        responder: ResponseChannel,
    },
    /// A `Sign` that needs no on-screen confirmation.
    AutoSign {
        req: Box<SignConfirmationRequest>,
    },
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

/// Runs deferred work on the main thread, whose stack is sized for curve math.
pub fn run_deferred(req: DeferredRequest) {
    match req.0 {
        Work::Commit { signer, responder } => {
            let response = match corisco_crypto_core::frost::frost_commit(&signer.roots.identity.private_key) {
                Ok((nonce, commitment)) => match (commitment.hiding().serialize(), commitment.binding().serialize()) {
                    (Ok(hiding), Ok(binding)) => {
                        let commitment_id = signer.next_commitment_id.fetch_add(1, Ordering::Relaxed);
                        signer.pending_commitments.lock().unwrap().insert(commitment_id, (nonce, commitment));
                        Response::Commit { commitment_id, hiding, binding }
                    }
                    (Err(e), _) | (_, Err(e)) => err(format!("{e:?}")),
                },
                Err(e) => err(format!("{e:?}")),
            };
            responder.send(&response);
        }
        Work::GetLeafPublicKey { signer, leaf_id, responder } => {
            let response = match signer.roots.derive_leaf_public_key(&leaf_id) {
                Ok(pk) => Response::PublicKey { public_key: pk.to_vec() },
                Err(e) => err(format!("{e:?}")),
            };
            responder.send(&response);
        }
        Work::SignSchnorrIdentity { signer, message, responder } => {
            let sig = corisco_crypto_core::sign_schnorr(&signer.roots.identity.private_key, &message);
            responder.send(&Response::Signature { signature: sig.to_bytes().to_vec() });
        }
        Work::SignEcdsaIdentity { signer, message, compact, responder } => {
            // DER unless `compact`: the SDK's auth flow omits `compact`, and spark_authn rejects
            // compact signatures.
            let response = match <[u8; 32]>::try_from(message.as_slice()) {
                Ok(digest) => {
                    let sig = corisco_crypto_core::sign_ecdsa_prehashed(&signer.roots.identity.private_key, &digest);
                    let signature = if compact { sig.to_bytes().to_vec() } else { sig.to_der().to_bytes().to_vec() };
                    Response::Signature { signature }
                }
                Err(_) => err("message must be exactly 32 bytes (a digest)"),
            };
            responder.send(&response);
        }
        Work::SubtractAndSplitSecretWithProofs { signer, first, second, threshold, num_shares, responder } => {
            let Some(claimed_leaf_id) = KeyDerivationRef::claim_leaf_id(&first, &second).map(str::to_owned) else {
                responder.send(&err("SubtractAndSplitSecretWithProofs only tweaks an incoming key onto a leaf"));
                return;
            };
            let response =
                match (resolve_private_key(&signer.roots, &first), resolve_private_key(&signer.roots, &second)) {
                    (Ok(a), Ok(b)) => {
                        let diff = corisco_crypto_core::subtract_private_keys(&a, &b);
                        match corisco_crypto_core::vss::split_secret_with_proofs(
                            &diff,
                            threshold as usize,
                            num_shares as usize,
                        ) {
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
        Work::DecryptEciesToPublicKey { signer, ciphertext, responder } => {
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
        Work::AutoSign { req } => complete_sign(req, true),
        Work::SubtractSplitAndEncrypt {
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
            if !crate::policy::is_ssp_identity_key(&receiver_public_key) {
                responder.send(&err("SubtractSplitAndEncrypt only encrypts to the Spark service provider"));
                return;
            }
            let response =
                match (resolve_private_key(&signer.roots, &first), resolve_private_key(&signer.roots, &second)) {
                    (Ok(a), Ok(b)) => {
                        let diff = corisco_crypto_core::subtract_private_keys(&a, &b);
                        match corisco_crypto_core::vss::split_secret_with_proofs(
                            &diff,
                            threshold as usize,
                            num_shares as usize,
                        ) {
                            Ok(shares) => match <[u8; 33]>::try_from(receiver_public_key.as_slice()) {
                                Ok(receiver_public_key) => {
                                    match corisco_crypto_core::encrypt_ecies(&b, &receiver_public_key) {
                                        Ok(secret_cipher) => Response::SubtractSplitAndEncrypt {
                                            shares: shares.into_iter().map(share_wire).collect(),
                                            secret_cipher,
                                        },
                                        Err(e) => err(e),
                                    }
                                }
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

/// Sends `payload` as MTU-sized indications, blocking until each one is confirmed.
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

/// Reassembles request frames and dispatches each complete message.
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

    // `None`: the deferred work sends the response later.
    if let Some(response) = response {
        responder.send(&response);
    }
}

/// Pairing passkey for the UI: with `DisplayOnly` the device itself must show it.
pub struct PasskeyDisplay {
    pub passkey: u32,
}

/// A peripheral can't advertise while connected, so these are the only two states.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BleStatus {
    Advertising,
    Connected,
}

/// Starts the GATT service and blocks forever. Failed sends to the UI are logged, never fatal.
pub fn run(
    roots: SparkKeyRoots,
    passkey_tx: mpsc::Sender<PasskeyDisplay>,
    confirm_tx: mpsc::Sender<Box<SignConfirmationRequest>>,
    work_tx: mpsc::Sender<DeferredRequest>,
    status_tx: mpsc::Sender<BleStatus>,
    require_confirmation: Arc<AtomicBool>,
    pairing_done_tx: mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let device = BLEDevice::take();

    // DisplayOnly: the phone asks for the passkey shown on this screen. `resolve_rpa` keeps bonded
    // iOS devices reconnecting across private-address rotation.
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
    // NimBLE stops advertising on connect and never resumes, so restart it on every disconnect.
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
            on_request_frame(
                &signer,
                &reassembly,
                &response_characteristic,
                &indicate_state,
                &indicate_cv,
                conn_handle,
                args.recv_data(),
            );
        });
    }

    BLEDevice::set_device_name(&device_name())?;
    advertising
        .lock()
        .set_data(BLEAdvertisementData::new().name(&device_name()).add_service_uuid(uuid(SERVICE_UUID)))?;
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

/// Random 6-digit pairing passkey from the hardware RNG.
fn rng_passkey() -> u32 {
    let mut buf = [0u8; 4];
    unsafe { esp_idf_svc::sys::esp_fill_random(buf.as_mut_ptr().cast(), buf.len()) };
    u32::from_le_bytes(buf) % 1_000_000
}
