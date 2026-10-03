//! Wire protocol between the phone app and the device's BLE signing service:
//! the `Request`/`Response` enums (postcard-encoded), the GATT UUIDs and the
//! protocol version. Kept free of any ESP-IDF dependency so it builds and
//! tests on a plain host -- that is what lets CI generate golden test
//! vectors (see `tests/vectors.rs`) that the mobile app's encoder/decoder is
//! checked against.
//!
//! Postcard encodes an enum variant by its declaration-order discriminant:
//! new `Request`/`Response` variants must only ever be **appended**.

use serde::{Deserialize, Serialize};

/// Bumped only on a wire-breaking change (an existing variant's bytes or
/// position changed, or one was removed). Appending a variant is additive
/// and does not bump it.
pub const PROTOCOL_VERSION: u16 = 1;

// Randomly generated, not derived from anything -- just needs to be
// distinct from any well-known service.
pub const SERVICE_UUID: u128 = 0x5f4b2a9e7c3d4e8fa1b6d09c2e7f4a5b;
pub const REQUEST_CHARACTERISTIC_UUID: u128 = 0x8e2c6f0a4b7d4c9e9a3f5d1e8b6c2a70;
pub const RESPONSE_CHARACTERISTIC_UUID: u128 = 0x3a9d7e1c5b4f4a8d8c2e6f0b9d3a7c50;

/// One Signing Operator's commitment, as part of a `Sign` request's
/// `statechain_commitments`. Kept as a flat list (not a map) since
/// `frost_secp256k1_tr::Identifier` doesn't implement `serde` -- the raw
/// identifier bytes get reconstructed via `Identifier::deserialize` when
/// handling the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatechainCommitment {
    pub identifier: Vec<u8>,
    pub hiding: Vec<u8>,
    pub binding: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Round 1 (`getRandomSigningCommitment`): no leaf key needed yet --
    /// `round1::commit`'s "secret" input is just entropy for a good random
    /// nonce, not a cryptographic binding to that specific key, so using
    /// the identity key here (always available, regardless of which leaf
    /// eventually gets spent) is fine.
    Commit,
    /// Round 2 (`signFrost`).
    Sign {
        /// id from a prior `Commit` response.
        commitment_id: u32,
        leaf_id: String,
        message: Vec<u8>,
        statechain_commitments: Vec<StatechainCommitment>,
        verifying_key: Vec<u8>,
        /// Present for Lightning payments that need a leaf swap first -- see
        /// `corisco_crypto_core::frost::frost_sign`'s doc comment.
        adaptor_public_key: Option<Vec<u8>>,
        /// `false` for the refund-transaction signatures a claim needs
        /// (internal statechain protocol plumbing -- a safety mechanism
        /// letting the new leaf owner unilaterally exit if operators go
        /// dark, not a transfer of value to anyone), `true` for a signature
        /// that actually spends/moves a leaf's value. Real hardware wallets
        /// don't ask you to confirm internal protocol plumbing, only real
        /// spends -- this field is what lets this one do the same. The app
        /// sets this from its own call-site context (see
        /// `ble-hardware-signer.ts`'s `withoutSpendConfirmation`), not from
        /// anything the device can infer on its own -- the wire protocol
        /// has no reliable way to distinguish "signing away my leaf" from
        /// "signing a refund tx for a leaf I'm keeping" from the message
        /// bytes alone.
        requires_confirmation: bool,
        /// App-asserted payment context for the confirm screen -- NOT
        /// independently verified against `message`, which is already a
        /// one-way sighash by the time it reaches this device (the SDK
        /// hands `SparkSigner.signFrost` only the hash, never the raw
        /// transaction it was computed from, so there is no way for this
        /// device to recompute or check it against these fields; a
        /// dishonest phone could show one thing here and sign something
        /// else entirely). Same trust model most software Lightning
        /// wallets already use, just surfaced on a second screen instead
        /// of only the phone's. Only meaningful when `requires_confirmation`
        /// is true; `None` falls back to the existing leaf id +
        /// message-fingerprint display.
        amount_sats: Option<u64>,
        destination: Option<String>,
    },
    GetIdentityPublicKey,
    GetDepositPublicKey,
    GetLeafPublicKey { leaf_id: String },
    SignSchnorrIdentity { message: Vec<u8> },
    /// Mirrors `signMessageWithIdentityKey(message, compact?)`; `message`
    /// here is always a 32-byte digest.
    SignEcdsaIdentity { message: Vec<u8>, compact: bool },
    /// `subtractAndSplitSecretWithProofsGivenDerivations` -- the leaf-
    /// ownership-transfer "key tweak" a claim needs to take ownership of an
    /// incoming leaf: subtract two derived private keys, then Shamir-split
    /// (with Feldman proofs) the difference for the Signing Operators.
    SubtractAndSplitSecretWithProofs {
        first: KeyDerivationRef,
        second: KeyDerivationRef,
        threshold: u32,
        num_shares: u32,
    },
    /// `decryptEcies` -- despite decrypting to a private key internally,
    /// the `SparkSigner` interface method returns only the *public* key of
    /// that decrypted value. Needed on the claim path specifically by
    /// `verifyPendingTransfer` (called from `claimTransferCore` before
    /// `subtractAndSplitSecretWithProofsGivenDerivations` even runs) --
    /// without it, claiming fails with "identityKey not initialized" (the
    /// base `DefaultSparkSigner`'s unimplemented fallback for this exact
    /// method).
    DecryptEciesToPublicKey { ciphertext: Vec<u8> },
    /// `subtractSplitAndEncrypt` -- the *sending*-side counterpart to
    /// `SubtractAndSplitSecretWithProofs`: needed when a payment's leaves
    /// don't sum to the exact invoice amount, so the SSP swaps them for
    /// ones that do (subtract two derived keys, Shamir-split the
    /// difference for the Signing Operators, and ECIES-encrypt the
    /// "second"/fresh key to the receiver so they can later claim it --
    /// exactly mirrors what `verifyPendingTransfer`/
    /// `SubtractAndSplitSecretWithProofs` do on the receiving end). Without
    /// it, paying an invoice that doesn't match an exact leaf denomination
    /// fails with "Private key not initialized" (`DefaultSparkSigner`'s
    /// unimplemented fallback for this exact method).
    SubtractSplitAndEncrypt {
        first: KeyDerivationRef,
        second: KeyDerivationRef,
        receiver_public_key: Vec<u8>,
        threshold: u32,
        num_shares: u32,
    },
}

/// Mirrors the SDK's `KeyDerivation` union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum KeyDerivationRef {
    Leaf { leaf_id: String },
    Deposit,
    StaticDeposit { idx: u32 },
    /// Raw ECIES ciphertext, decrypted with the identity key.
    Ecies { ciphertext: Vec<u8> },
    Random,
}

/// Wire format for `corisco_crypto_core::vss::VerifiableSecretShare`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShareWire {
    pub threshold: u32,
    pub index: u32,
    pub share: Vec<u8>,
    pub proofs: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Commit { commitment_id: u32, hiding: Vec<u8>, binding: Vec<u8> },
    Sign { signature_share: Vec<u8> },
    PublicKey { public_key: Vec<u8> },
    Signature { signature: Vec<u8> },
    Error { message: String },
    Shares { shares: Vec<ShareWire> },
    SubtractSplitAndEncrypt { shares: Vec<ShareWire>, secret_cipher: Vec<u8> },
}
