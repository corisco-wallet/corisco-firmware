//! BLE wire protocol shared by the firmware and the phone app. Free of ESP-IDF dependencies so
//! the golden vectors (`tests/vectors.rs`) the app is tested against build on the host.
//!
//! Postcard encodes enum variants by declaration order: only ever append new variants.

use serde::{Deserialize, Serialize};

/// Bumped only on a wire-breaking change (a variant's bytes or position changed, or one was
/// removed); appending a variant doesn't bump it.
pub const PROTOCOL_VERSION: u16 = 1;

pub const SERVICE_UUID: u128 = 0x5f4b2a9e7c3d4e8fa1b6d09c2e7f4a5b;
pub const REQUEST_CHARACTERISTIC_UUID: u128 = 0x8e2c6f0a4b7d4c9e9a3f5d1e8b6c2a70;
pub const RESPONSE_CHARACTERISTIC_UUID: u128 = 0x3a9d7e1c5b4f4a8d8c2e6f0b9d3a7c50;

/// One Signing Operator's commitment; a list, not a map, because `Identifier` has no serde.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatechainCommitment {
    pub identifier: Vec<u8>,
    pub hiding: Vec<u8>,
    pub binding: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// FROST round 1. Uses the identity key: the commit secret only seeds nonce entropy and isn't
    /// bound to the leaf being spent.
    Commit,
    /// Round 2 (`signFrost`).
    Sign {
        /// id from a prior `Commit` response.
        commitment_id: u32,
        leaf_id: String,
        message: Vec<u8>,
        statechain_commitments: Vec<StatechainCommitment>,
        verifying_key: Vec<u8>,
        /// Set for Lightning payments that need a leaf swap first.
        adaptor_public_key: Option<Vec<u8>>,
        /// `true` for leaf spends, `false` for claim refund-tx signatures. `false` alone can't skip
        /// the prompt: the device must also have seen the leaf being claimed.
        requires_confirmation: bool,
        /// App-asserted, NOT verified: `message` is already a sighash, so a dishonest phone could
        /// show one amount and sign another. `None` falls back to leaf id and message fingerprint.
        amount_sats: Option<u64>,
        destination: Option<String>,
    },
    GetIdentityPublicKey,
    GetDepositPublicKey,
    GetLeafPublicKey {
        leaf_id: String,
    },
    SignSchnorrIdentity {
        message: Vec<u8>,
    },
    /// Mirrors `signMessageWithIdentityKey(message, compact?)`; `message`
    /// here is always a 32-byte digest.
    SignEcdsaIdentity {
        message: Vec<u8>,
        compact: bool,
    },
    /// Claim key tweak (`subtractAndSplitSecretWithProofsGivenDerivations`): subtract two derived
    /// keys and Shamir-split the difference, with Feldman proofs, for the Signing Operators.
    SubtractAndSplitSecretWithProofs {
        first: KeyDerivationRef,
        second: KeyDerivationRef,
        threshold: u32,
        num_shares: u32,
    },
    /// `decryptEcies`: returns only the public key of the decrypted private key. The SDK's claim
    /// verification needs it.
    DecryptEciesToPublicKey {
        ciphertext: Vec<u8>,
    },
    /// `subtractSplitAndEncrypt`: the sending-side leaf swap, like `SubtractAndSplitSecretWithProofs`
    /// plus the new key ECIES-encrypted to the receiver.
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
    Leaf {
        leaf_id: String,
    },
    Deposit,
    StaticDeposit {
        idx: u32,
    },
    /// Raw ECIES ciphertext, decrypted with the identity key.
    Ecies {
        ciphertext: Vec<u8>,
    },
    Random,
}

impl KeyDerivationRef {
    /// The only shape `SubtractSplitAndEncrypt` legitimately uses: a leaf key swapped for a fresh random one.
    pub fn is_leaf_swap(first: &Self, second: &Self) -> bool {
        matches!((first, second), (Self::Leaf { .. }, Self::Random))
    }

    /// The leaf a claim takes ownership of: an incoming ECIES key tweaked onto a local leaf key.
    pub fn claim_leaf_id<'a>(first: &Self, second: &'a Self) -> Option<&'a str> {
        match (first, second) {
            (Self::Ecies { .. }, Self::Leaf { leaf_id }) => Some(leaf_id),
            _ => None,
        }
    }
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

#[cfg(test)]
mod key_derivation_tests {
    use super::KeyDerivationRef::*;
    use super::*;

    #[test]
    fn leaf_swap_accepts_only_leaf_then_random() {
        let leaf = || Leaf { leaf_id: "a".into() };
        assert!(KeyDerivationRef::is_leaf_swap(&leaf(), &Random));
        for second in [leaf(), Deposit, StaticDeposit { idx: 0 }, Ecies { ciphertext: vec![] }] {
            assert!(!KeyDerivationRef::is_leaf_swap(&leaf(), &second));
        }
        for first in [Deposit, StaticDeposit { idx: 0 }, Ecies { ciphertext: vec![] }, Random] {
            assert!(!KeyDerivationRef::is_leaf_swap(&first, &Random));
        }
    }

    #[test]
    fn claim_leaf_id_requires_ecies_then_leaf() {
        let ecies = || Ecies { ciphertext: vec![] };
        let leaf = || Leaf { leaf_id: "a".into() };
        assert_eq!(KeyDerivationRef::claim_leaf_id(&ecies(), &leaf()), Some("a"));
        assert_eq!(KeyDerivationRef::claim_leaf_id(&leaf(), &leaf()), None);
        assert_eq!(KeyDerivationRef::claim_leaf_id(&Random, &leaf()), None);
        assert_eq!(KeyDerivationRef::claim_leaf_id(&ecies(), &Random), None);
        assert_eq!(KeyDerivationRef::claim_leaf_id(&ecies(), &Deposit), None);
    }
}
