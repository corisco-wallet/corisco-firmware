//! Boot-time crypto self-tests. Board-agnostic: exercises `corisco-crypto-core`
//! against real hardware (timing, actual elliptic-curve math) without
//! touching display/touch/BLE, so every board crate can call these from its
//! own `main()` the same way.

/// Isolates the cost of a single SHA512 compression: `Sha512::digest` on a
/// <=111-byte message pads to exactly one 128-byte block, so N calls here
/// is N compressions. Used to find out whether PBKDF2's ~4096-compression
/// cost (2048 rounds x 2 HMAC-SHA512 calls) is "software SHA512 is just
/// this slow on Xtensa" or something worse than that.
pub fn bench_sha512_single_block() {
    use log::info;
    use sha2::{Digest, Sha512};

    const ITERATIONS: u32 = 200;
    let data = [0xABu8; 64];
    let mut sink = 0u8;

    let t0 = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    for _ in 0..ITERATIONS {
        let digest = Sha512::digest(data);
        sink ^= digest[0];
    }
    let t1 = unsafe { esp_idf_svc::sys::esp_timer_get_time() };

    let elapsed_us = t1 - t0;
    info!(
        "sha512 bench: {ITERATIONS} single-block digests in {elapsed_us} us ({} us/digest, sink={sink})",
        elapsed_us / ITERATIONS as i64
    );
}

/// Exercises `corisco_crypto_core::frost`'s round1/round2 against real
/// elliptic-curve math on real hardware, end to end: simulates a 3-of-5
/// Signing-Operator group locally (standing in for the real network, which
/// doesn't exist yet), signs with a real derived LEAF key exactly as
/// `frost_commit`/`frost_sign` would be called over BLE, aggregates, and
/// verifies the resulting signature. Mirrors `crypto-core`'s
/// `user_signature_share_aggregates_into_valid_signature` host test, but
/// running (and timed) on the actual target chip.
pub fn frost_self_test(roots: &corisco_crypto_core::SparkKeyRoots) -> anyhow::Result<()> {
    use frost_secp256k1_tr::{
        aggregate_with_tweak,
        keys::{generate_with_dealer, EvenY, IdentifierList, KeyPackage as FrostKeyPackage, PublicKeyPackage, Tweak},
        round1, round2, Identifier, SigningPackage, VerifyingKey,
    };
    use log::info;
    use rand_core::OsRng;
    use std::collections::{BTreeMap, BTreeSet};

    let mut rng = OsRng;
    let merkle_root: Vec<u8> = vec![];
    let message = b"spark on-device FROST self-test transaction";

    let leaf_key = roots.derive_leaf_key("018f3e2a-frost-self-test-0000-000000000000")?;

    // Simulated Signing-Operator group: rerolled (not the device's own key)
    // until the combined key is even-Y, matching the deployed invariant
    // that a leaf's combined key is established even-Y at creation time --
    // see crypto-core's frost.rs test for why.
    let leaf_signing_share = frost_secp256k1_tr::keys::SigningShare::deserialize(&leaf_key)?;
    let leaf_verifying_share = frost_secp256k1_tr::keys::VerifyingShare::from(leaf_signing_share);

    let (se_key_packages, combined_vk) = loop {
        let (se_shares, se_pubkey_pkg) = generate_with_dealer(5, 3, IdentifierList::Default, &mut rng)?;
        let combined = VerifyingKey::new(
            se_pubkey_pkg.verifying_key().to_element() + leaf_verifying_share.to_element(),
        );
        if combined.has_even_y() {
            let se_key_packages: BTreeMap<Identifier, FrostKeyPackage> = se_shares
                .into_iter()
                .map(|(id, share)| Ok::<_, anyhow::Error>((id, FrostKeyPackage::try_from(share)?)))
                .collect::<anyhow::Result<_>>()?;
            break (se_key_packages, combined);
        }
    };

    let t0 = unsafe { esp_idf_svc::sys::esp_timer_get_time() };

    // --- Round 1 ---
    let se_signers: Vec<Identifier> = se_key_packages.keys().take(3).cloned().collect();
    let mut se_nonces = BTreeMap::new();
    let mut se_commitments: BTreeMap<Identifier, corisco_crypto_core::frost::SigningCommitments> = BTreeMap::new();
    for id in &se_signers {
        let kp = &se_key_packages[id];
        let (nonce, commitment) = round1::commit(kp.signing_share(), &mut rng);
        se_nonces.insert(*id, nonce);
        se_commitments.insert(*id, commitment);
    }
    let (device_nonce, device_commitment) = corisco_crypto_core::frost::frost_commit(&leaf_key)?;

    let t1 = unsafe { esp_idf_svc::sys::esp_timer_get_time() };

    // --- Round 2 ---
    let signing_package = SigningPackage::new_with_adaptor(
        {
            let mut c = se_commitments.clone();
            c.insert(corisco_crypto_core::frost::user_identifier(), device_commitment);
            c
        },
        Some(vec![
            se_commitments.keys().cloned().collect(),
            BTreeSet::from([corisco_crypto_core::frost::user_identifier()]),
        ]),
        message,
        None,
    );
    let mut all_shares = BTreeMap::new();
    for id in &se_signers {
        let orig = &se_key_packages[id];
        let kp = FrostKeyPackage::new(
            *orig.identifier(),
            *orig.signing_share(),
            *orig.verifying_share(),
            combined_vk,
            *orig.min_signers(),
        )
        .tweak(Some(merkle_root.as_slice()));
        all_shares.insert(*id, round2::sign(&signing_package, &se_nonces[id], &kp)?);
    }

    let device_share_bytes = corisco_crypto_core::frost::frost_sign(
        message,
        &leaf_key,
        &device_nonce,
        device_commitment,
        se_commitments,
        &combined_vk.serialize()?,
        None,
    )?;

    let t2 = unsafe { esp_idf_svc::sys::esp_timer_get_time() };

    all_shares.insert(
        corisco_crypto_core::frost::user_identifier(),
        round2::SignatureShare::deserialize(&device_share_bytes)?,
    );

    // --- Aggregate + verify (phone-side steps in reality; done here too so
    // the on-device numbers mean something end to end) ---
    let mut verifying_shares: BTreeMap<_, _> = se_key_packages
        .iter()
        .map(|(id, kp)| (*id, *kp.verifying_share()))
        .collect();
    let device_signing_share = frost_secp256k1_tr::keys::SigningShare::deserialize(&leaf_key)?;
    verifying_shares.insert(
        corisco_crypto_core::frost::user_identifier(),
        frost_secp256k1_tr::keys::VerifyingShare::from(device_signing_share),
    );
    let public_package = PublicKeyPackage::new(verifying_shares, combined_vk, None);

    let signature = aggregate_with_tweak(&signing_package, &all_shares, &public_package, Some(&merkle_root))?;
    let tweaked_vk = *public_package.tweak(Some(merkle_root.as_slice())).verifying_key();
    tweaked_vk
        .verify(message, &signature)
        .map_err(|e| anyhow::anyhow!("FROST self-test signature failed to verify: {e:?}"))?;

    info!(
        "frost self-test: round1 {} us, round2 {} us, signature verified ok",
        t1 - t0,
        t2 - t1
    );

    Ok(())
}
