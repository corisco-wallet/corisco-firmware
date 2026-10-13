//! Per-device secret derived inside the HMAC peripheral from an eFuse key
//! block. The key itself is read-protected and never reaches software.

use esp_idf_svc::sys::{
    esp_efuse_block_t_EFUSE_BLK_KEY0, esp_efuse_find_purpose,
    esp_efuse_purpose_t_ESP_EFUSE_KEY_PURPOSE_HMAC_UP,
};

extern "C" {
    // Not in esp-idf-sys's generated bindings; esp_security is linked by default.
    fn esp_hmac_calculate(key_id: u32, message: *const u8, len: usize, hmac: *mut u8) -> i32;
}

const DERIVATION_MESSAGE: &[u8] = b"corisco/device-secret/v1";

/// `None` on boards without a provisioned HMAC key block (dev boards).
pub fn secret() -> Option<[u8; 32]> {
    let mut block = 0;
    let found = unsafe {
        esp_efuse_find_purpose(esp_efuse_purpose_t_ESP_EFUSE_KEY_PURPOSE_HMAC_UP, &mut block)
    };
    if !found {
        return None;
    }
    let mut out = [0u8; 32];
    let rc = unsafe {
        esp_hmac_calculate(
            block - esp_efuse_block_t_EFUSE_BLK_KEY0,
            DERIVATION_MESSAGE.as_ptr(),
            DERIVATION_MESSAGE.len(),
            out.as_mut_ptr(),
        )
    };
    (rc == 0).then_some(out)
}
