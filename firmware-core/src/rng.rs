//! Hardware TRNG entropy for key material. `esp_fill_random` is ESP-IDF's cryptographic-quality
//! source; crypto-core never draws its own randomness for the seed, callers supply it.

/// 128 bits of hardware-TRNG entropy -- a 12-word mnemonic's worth.
pub fn device_entropy_128() -> [u8; 16] {
    let mut buf = [0u8; 16];
    fill(&mut buf);
    buf
}

/// 256 bits of hardware-TRNG entropy -- a 24-word mnemonic's worth.
pub fn device_entropy_256() -> [u8; 32] {
    let mut buf = [0u8; 32];
    fill(&mut buf);
    buf
}

/// A random 16-byte salt (for `seed_lock`'s KDF salt).
pub fn random_salt_16() -> [u8; 16] {
    let mut buf = [0u8; 16];
    fill(&mut buf);
    buf
}

/// A random 12-byte AES-GCM nonce (for `seed_lock`'s ciphertext nonce).
pub fn random_nonce_12() -> [u8; 12] {
    let mut buf = [0u8; 12];
    fill(&mut buf);
    buf
}

fn fill(buf: &mut [u8]) {
    unsafe {
        esp_idf_svc::sys::esp_fill_random(buf.as_mut_ptr().cast(), buf.len());
    }
}
