//! NVS-backed, PIN-encrypted seed storage plus the PIN-attempt/lockout
//! counter. Wraps `esp_idf_svc::nvs::EspNvs` -- see `main.rs` for why the
//! `EspDefaultNvsPartition` is taken once there and cloned into both this
//! module and `ble::run` (it's a process-wide singleton handle).

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sys::EspError;
use signer_core::seed_lock::EncryptedSeed;

const NAMESPACE: &str = "signer";
const KEY_SEED_BLOB: &str = "seed_blob";
const KEY_FAIL_COUNT: &str = "fail_count";
const KEY_REQUIRE_CONFIRMATION: &str = "req_confirm";

/// Lockout threshold: the encrypted seed is wiped after this many
/// consecutive wrong-PIN attempts (Ledger/Trezor-style).
pub const MAX_PIN_ATTEMPTS: u8 = 10;

// salt(16) + nonce(12) + ciphertext. Ciphertext length is NOT fixed at the
// type level (`EncryptedSeed.ciphertext: Vec<u8>`), but in practice is
// always 64-byte-seed + 16-byte GCM tag = 80 bytes, since this only ever
// encrypts a `[u8; 64]` seed. Stored length-prefixed anyway (2 bytes, LE)
// rather than hardcoding 80 -- cheap insurance against `seed_lock`'s
// ciphertext framing changing later, and this is a one-time few-hundred-
// byte blob, not a hot path.
fn encode(enc: &EncryptedSeed) -> Vec<u8> {
    let mut buf = Vec::with_capacity(16 + 12 + 2 + enc.ciphertext.len());
    buf.extend_from_slice(&enc.salt);
    buf.extend_from_slice(&enc.nonce);
    buf.extend_from_slice(&(enc.ciphertext.len() as u16).to_le_bytes());
    buf.extend_from_slice(&enc.ciphertext);
    buf
}

fn decode(buf: &[u8]) -> Option<EncryptedSeed> {
    if buf.len() < 16 + 12 + 2 {
        return None;
    }
    let salt: [u8; 16] = buf[0..16].try_into().ok()?;
    let nonce: [u8; 12] = buf[16..28].try_into().ok()?;
    let ct_len = u16::from_le_bytes([buf[28], buf[29]]) as usize;
    let ciphertext = buf.get(30..30 + ct_len)?.to_vec();
    Some(EncryptedSeed { salt, nonce, ciphertext })
}

pub struct Storage {
    nvs: EspNvs<NvsDefault>,
}

impl Storage {
    pub fn init(partition: EspDefaultNvsPartition) -> Result<Self, EspError> {
        let nvs = EspNvs::new(partition, NAMESPACE, true)?;
        Ok(Self { nvs })
    }

    pub fn is_provisioned(&self) -> Result<bool, EspError> {
        Ok(self.nvs.blob_len(KEY_SEED_BLOB)?.is_some())
    }

    pub fn store(&self, enc: &EncryptedSeed) -> Result<(), EspError> {
        self.nvs.set_blob(KEY_SEED_BLOB, &encode(enc))?;
        self.reset_failures()?;
        Ok(())
    }

    pub fn load(&self) -> Result<Option<EncryptedSeed>, EspError> {
        let Some(len) = self.nvs.blob_len(KEY_SEED_BLOB)? else {
            return Ok(None);
        };
        let mut buf = vec![0u8; len];
        let Some(bytes) = self.nvs.get_blob(KEY_SEED_BLOB, &mut buf)? else {
            return Ok(None);
        };
        Ok(decode(bytes))
    }

    /// Records a wrong-PIN attempt and returns the new count. Callers
    /// should wipe once this reaches `MAX_PIN_ATTEMPTS`.
    pub fn record_failure(&self) -> Result<u8, EspError> {
        let count = self.nvs.get_u8(KEY_FAIL_COUNT)?.unwrap_or(0).saturating_add(1);
        self.nvs.set_u8(KEY_FAIL_COUNT, count)?;
        Ok(count)
    }

    pub fn reset_failures(&self) -> Result<(), EspError> {
        self.nvs.remove(KEY_FAIL_COUNT)?;
        Ok(())
    }

    /// Whether a real `Sign` still gates on the on-screen confirm prompt --
    /// see `ble.rs`'s `Signer.require_confirmation`, which this seeds at
    /// boot and `main.rs`'s Settings-screen wiring keeps in sync
    /// afterward. Defaults to `true` (the safe choice) when never
    /// explicitly set, same convention as `is_provisioned`/`load` treating
    /// "key absent" as the safe/empty case rather than an error.
    pub fn get_require_confirmation(&self) -> Result<bool, EspError> {
        Ok(self.nvs.get_u8(KEY_REQUIRE_CONFIRMATION)?.map(|v| v != 0).unwrap_or(true))
    }

    pub fn set_require_confirmation(&self, enabled: bool) -> Result<(), EspError> {
        self.nvs.set_u8(KEY_REQUIRE_CONFIRMATION, enabled as u8)?;
        Ok(())
    }

    /// Erases the encrypted seed, attempt counter, and settings -- used
    /// both by the lockout policy (`record_failure` reaching
    /// `MAX_PIN_ATTEMPTS`) and as a manual factory-reset affordance, since
    /// a device that can only ever be wiped by locking itself out would be
    /// a trap during development. Settings are included so a wiped-and-
    /// re-onboarded device always starts back at the safe defaults (in
    /// particular, `require_confirmation` back to `true`) rather than
    /// silently inheriting a previous owner's relaxed settings.
    pub fn wipe(&self) -> Result<(), EspError> {
        self.nvs.remove(KEY_SEED_BLOB)?;
        self.nvs.remove(KEY_FAIL_COUNT)?;
        self.nvs.remove(KEY_REQUIRE_CONFIRMATION)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trips() {
        let enc = EncryptedSeed { salt: [0x11; 16], nonce: [0x22; 12], ciphertext: vec![0x33; 80] };
        let decoded = decode(&encode(&enc)).expect("should decode");
        assert_eq!(decoded.salt, enc.salt);
        assert_eq!(decoded.nonce, enc.nonce);
        assert_eq!(decoded.ciphertext, enc.ciphertext);
    }

    #[test]
    fn decode_rejects_truncated_input() {
        assert!(decode(&[0u8; 10]).is_none());
    }
}
