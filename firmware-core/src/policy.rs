/// Spark service provider identity keys (mainnet, regtest) from the Spark SDK's wallet config. Rotation needs a firmware update.
const SSP_IDENTITY_KEYS: [&str; 2] = [
    "023e33e2920326f64ea31058d44777442d97d7d5cbfcf54e3060bc1695e5261c93",
    "022bf283544b16c0622daecb79422007d167eca6ce9f0c98c0c49833b1f7170bfe",
];

pub fn is_ssp_identity_key(public_key: &[u8]) -> bool {
    let hex: String = public_key.iter().map(|b| format!("{b:02x}")).collect();
    SSP_IDENTITY_KEYS.contains(&hex.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn accepts_both_pinned_keys_only() {
        for key in SSP_IDENTITY_KEYS {
            assert!(is_ssp_identity_key(&bytes(key)));
        }
        let mut other = bytes(SSP_IDENTITY_KEYS[0]);
        other[32] ^= 1;
        assert!(!is_ssp_identity_key(&other));
        assert!(!is_ssp_identity_key(&[]));
        assert!(!is_ssp_identity_key(&bytes(SSP_IDENTITY_KEYS[0])[..32]));
    }
}
