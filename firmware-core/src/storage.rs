//! PIN-encrypted seed storage in NVS, plus the wrong-PIN counter and settings.

use corisco_crypto_core::seed_lock::EncryptedSeed;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sys::EspError;

const NAMESPACE: &str = "signer";
const KEY_SEED_BLOB: &str = "seed_blob";
const KEY_FAIL_COUNT: &str = "fail_count";
const KEY_REQUIRE_CONFIRMATION: &str = "req_confirm";

/// The encrypted seed is wiped after this many consecutive wrong PINs.
pub const MAX_PIN_ATTEMPTS: u8 = 10;

// salt(16) | nonce(12) | ciphertext len (u16 LE) | ciphertext. Length-prefixed so a change in
// seed_lock's ciphertext size can't break stored blobs.
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

    /// Whether `Sign` requires on-screen confirmation; `true` (the safe default) when never set.
    pub fn get_require_confirmation(&self) -> Result<bool, EspError> {
        Ok(self.nvs.get_u8(KEY_REQUIRE_CONFIRMATION)?.map(|v| v != 0).unwrap_or(true))
    }

    pub fn set_require_confirmation(&self, enabled: bool) -> Result<(), EspError> {
        self.nvs.set_u8(KEY_REQUIRE_CONFIRMATION, enabled as u8)?;
        Ok(())
    }

    /// Erases seed, attempt counter and settings, so a re-onboarded device starts from safe
    /// defaults instead of a previous owner's relaxed ones.
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
