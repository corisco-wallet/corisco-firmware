//! Golden wire vectors: the exact postcard bytes for a representative value
//! of every `Request`/`Response` variant, committed as
//! `protocol/vectors.json`. The mobile app's TypeScript encoder/decoder is
//! tested against that file, so this test failing means the wire format
//! changed and the app has to follow.
//!
//! Regenerate after an intentional change:
//! `UPDATE_VECTORS=1 cargo test -p corisco-protocol --test vectors`

use corisco_protocol::*;
use serde::Serialize;
use serde_json::{json, Value};

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| seed.wrapping_add(i as u8)).collect()
}

fn derivations() -> Vec<(&'static str, KeyDerivationRef)> {
    vec![
        ("Leaf", KeyDerivationRef::Leaf { leaf_id: "leaf-1".into() }),
        ("Deposit", KeyDerivationRef::Deposit),
        ("StaticDeposit", KeyDerivationRef::StaticDeposit { idx: 300 }),
        ("Ecies", KeyDerivationRef::Ecies { ciphertext: bytes(5, 7) }),
        ("Random", KeyDerivationRef::Random),
    ]
}

fn share() -> ShareWire {
    ShareWire { threshold: 2, index: 1, share: bytes(32, 1), proofs: vec![bytes(33, 2), bytes(33, 3)] }
}

fn requests() -> Vec<(String, Request)> {
    let sc = |i: u8| StatechainCommitment { identifier: bytes(32, i), hiding: bytes(33, i + 1), binding: bytes(33, i + 2) };
    let sign = |adaptor, amount, dest: Option<&str>, confirm| Request::Sign {
        commitment_id: 70_000,
        leaf_id: "leaf-1".into(),
        message: bytes(32, 9),
        statechain_commitments: vec![sc(10), sc(20)],
        verifying_key: bytes(33, 4),
        adaptor_public_key: adaptor,
        requires_confirmation: confirm,
        amount_sats: amount,
        destination: dest.map(String::from),
    };
    let mut v: Vec<(String, Request)> = vec![
        ("Commit".into(), Request::Commit),
        ("Sign/minimal".into(), sign(None, None, None, false)),
        // > u32::MAX sats: guards the app's u64 varint against 32-bit truncation.
        ("Sign/full".into(), sign(Some(bytes(33, 5)), Some(5_000_000_000), Some("bc1qexample"), true)),
        ("GetIdentityPublicKey".into(), Request::GetIdentityPublicKey),
        ("GetDepositPublicKey".into(), Request::GetDepositPublicKey),
        ("GetLeafPublicKey".into(), Request::GetLeafPublicKey { leaf_id: "leaf-1".into() }),
        ("SignSchnorrIdentity".into(), Request::SignSchnorrIdentity { message: bytes(32, 1) }),
        ("SignEcdsaIdentity/compact".into(), Request::SignEcdsaIdentity { message: bytes(32, 1), compact: true }),
        ("SignEcdsaIdentity/der".into(), Request::SignEcdsaIdentity { message: bytes(32, 1), compact: false }),
        ("DecryptEciesToPublicKey".into(), Request::DecryptEciesToPublicKey { ciphertext: bytes(200, 3) }),
    ];
    for (name, d) in derivations() {
        v.push((
            format!("SubtractAndSplitSecretWithProofs/{name}"),
            Request::SubtractAndSplitSecretWithProofs { first: d.clone(), second: KeyDerivationRef::Deposit, threshold: 2, num_shares: 3 },
        ));
        v.push((
            format!("SubtractSplitAndEncrypt/{name}"),
            Request::SubtractSplitAndEncrypt {
                first: KeyDerivationRef::Random,
                second: d,
                receiver_public_key: bytes(33, 6),
                threshold: 2,
                num_shares: 3,
            },
        ));
    }
    // Varint boundary: lengths of 127/128 bytes are 1 vs 2 length bytes.
    v.push(("SignSchnorrIdentity/len127".into(), Request::SignSchnorrIdentity { message: bytes(127, 0) }));
    v.push(("SignSchnorrIdentity/len128".into(), Request::SignSchnorrIdentity { message: bytes(128, 0) }));
    v
}

fn responses() -> Vec<(String, Response)> {
    vec![
        ("Commit".into(), Response::Commit { commitment_id: 70_000, hiding: bytes(33, 1), binding: bytes(33, 2) }),
        ("Sign".into(), Response::Sign { signature_share: bytes(64, 1) }),
        ("PublicKey".into(), Response::PublicKey { public_key: bytes(33, 2) }),
        ("Signature".into(), Response::Signature { signature: bytes(64, 3) }),
        ("Error".into(), Response::Error { message: "unknown commitment_id: 7".into() }),
        ("Shares".into(), Response::Shares { shares: vec![share(), share()] }),
        ("Shares/empty".into(), Response::Shares { shares: vec![] }),
        ("SubtractSplitAndEncrypt".into(), Response::SubtractSplitAndEncrypt { shares: vec![share()], secret_cipher: bytes(100, 4) }),
    ]
}

// Exhaustive on purpose (no wildcard): adding a variant without adding a
// vector for it above fails to compile here.
#[allow(dead_code)]
fn every_variant_needs_a_vector(req: &Request, resp: &Response) {
    match req {
        Request::Commit
        | Request::Sign { .. }
        | Request::GetIdentityPublicKey
        | Request::GetDepositPublicKey
        | Request::GetLeafPublicKey { .. }
        | Request::SignSchnorrIdentity { .. }
        | Request::SignEcdsaIdentity { .. }
        | Request::SubtractAndSplitSecretWithProofs { .. }
        | Request::DecryptEciesToPublicKey { .. }
        | Request::SubtractSplitAndEncrypt { .. } => {}
    }
    match resp {
        Response::Commit { .. }
        | Response::Sign { .. }
        | Response::PublicKey { .. }
        | Response::Signature { .. }
        | Response::Error { .. }
        | Response::Shares { .. }
        | Response::SubtractSplitAndEncrypt { .. } => {}
    }
}

fn entry<T: Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug>(name: &str, value: &T) -> Value {
    let encoded = postcard::to_allocvec(value).unwrap();
    let decoded: T = postcard::from_bytes(&encoded).unwrap();
    assert_eq!(&decoded, value, "{name}: postcard round trip");
    json!({ "name": name, "value": serde_json::to_value(value).unwrap(), "hex": hex::encode(encoded) })
}

fn generate() -> Value {
    let uuid = |u: u128| {
        let h = format!("{u:032x}");
        format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    };
    json!({
        "protocol": PROTOCOL_VERSION,
        "uuids": {
            "service": uuid(SERVICE_UUID),
            "request": uuid(REQUEST_CHARACTERISTIC_UUID),
            "response": uuid(RESPONSE_CHARACTERISTIC_UUID),
        },
        "requests": requests().iter().map(|(n, v)| entry(n, v)).collect::<Vec<_>>(),
        "responses": responses().iter().map(|(n, v)| entry(n, v)).collect::<Vec<_>>(),
    })
}

#[test]
fn committed_vectors_are_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../protocol/vectors.json");
    let fresh = serde_json::to_string_pretty(&generate()).unwrap() + "\n";
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::write(path, &fresh).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(path).expect("protocol/vectors.json missing -- run with UPDATE_VECTORS=1");
    assert!(
        committed == fresh,
        "protocol/vectors.json is stale: the wire format changed. If intentional, regenerate with\n  \
         UPDATE_VECTORS=1 cargo test -p corisco-protocol --test vectors\n\
         and update the mobile app. Breaking changes (existing variant changed/removed) also need a PROTOCOL_VERSION bump."
    );
}
