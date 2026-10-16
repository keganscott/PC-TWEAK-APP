//! Signed data files (plan section 8, profile packs; the same check will serve
//! the offline licence tokens of plan section 9): a JSON envelope around a
//! payload and an Ed25519 signature, checked against public keys built into
//! the app before anything in the payload is read.
//!
//! ```text
//! {"format":"peaktweaks-signed-1","purpose":"profile-pack","key":"<key id>",
//!  "payload":"<base64>","signature":"<base64, 64 bytes>"}
//! ```
//!
//! The signed bytes are `peaktweaks-signed-1\n<purpose>\n<payload>`, so a
//! signature made for one purpose (a licence) never passes as another (a
//! pack). Verification is strict (`verify_strict`: no small-order keys, no
//! malleable signatures). Only the public half of a key is ever in this
//! repository; the private half stays with whoever signs (NOTES N138).

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;

pub const FORMAT: &str = "peaktweaks-signed-1";

/// A public key the app trusts, under the id envelopes name it by.
pub struct TrustedKey {
    pub id: &'static str,
    pub public: [u8; 32],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    purpose: String,
    key: String,
    payload: String,
    signature: String,
}

/// Why a signed file was not used, in words for the activity log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

fn refused(why: impl Into<String>) -> Refused {
    Refused(why.into())
}

/// The bytes a signature covers.
pub fn signed_bytes(purpose: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FORMAT.len() + purpose.len() + payload.len() + 2);
    out.extend_from_slice(FORMAT.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(purpose.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(payload);
    out
}

/// The payload of `text` when it is an envelope for `purpose` signed by one
/// of `keys`; nothing of the payload is returned otherwise.
pub fn verify(text: &str, purpose: &str, keys: &[TrustedKey]) -> Result<Vec<u8>, Refused> {
    let env: Envelope =
        serde_json::from_str(text).map_err(|e| refused(format!("not a signed PeakTweaks file ({e})")))?;
    if env.format != FORMAT {
        return Err(refused(format!("unknown format {:?}", env.format)));
    }
    if env.purpose != purpose {
        return Err(refused(format!("signed for {:?}, not {purpose:?}", env.purpose)));
    }
    let key = keys.iter().find(|k| k.id == env.key).ok_or_else(|| {
        refused(format!(
            "signed with key {:?}, which this build does not trust",
            env.key
        ))
    })?;
    let public = VerifyingKey::from_bytes(&key.public).map_err(|_| refused("a built-in key is not a valid key"))?;
    let payload = STANDARD
        .decode(env.payload.as_bytes())
        .map_err(|_| refused("the payload is not base64"))?;
    let sig: [u8; 64] = STANDARD
        .decode(env.signature.as_bytes())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| refused("the signature is not 64 bytes of base64"))?;
    public
        .verify_strict(&signed_bytes(purpose, &payload), &Signature::from_bytes(&sig))
        .map_err(|_| refused("the signature does not match"))?;
    Ok(payload)
}

/// Make an envelope (`examples/sign_pack.rs`, tests). The signing key never
/// comes from this repository.
pub fn sign(seed: &[u8; 32], key_id: &str, purpose: &str, payload: &[u8]) -> String {
    use ed25519_dalek::Signer as _;
    let signing = ed25519_dalek::SigningKey::from_bytes(seed);
    let sig = signing.sign(&signed_bytes(purpose, payload));
    serde_json::json!({
        "format": FORMAT,
        "purpose": purpose,
        "key": key_id,
        "payload": STANDARD.encode(payload),
        "signature": STANDARD.encode(sig.to_bytes()),
    })
    .to_string()
}

/// The public half of a signing seed.
pub fn public_key(seed: &[u8; 32]) -> [u8; 32] {
    ed25519_dalek::SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A key for tests only; its seed is public, so nothing it signs is
    /// trusted outside tests.
    pub(crate) const TEST_SEED: [u8; 32] = [7; 32];

    pub(crate) fn test_keys() -> Vec<TrustedKey> {
        vec![TrustedKey {
            id: "test",
            public: public_key(&TEST_SEED),
        }]
    }

    #[test]
    fn a_signed_payload_comes_back_only_for_its_purpose_and_key() {
        let keys = test_keys();
        let text = sign(&TEST_SEED, "test", "profile-pack", b"{\"x\":1}");
        assert_eq!(verify(&text, "profile-pack", &keys).unwrap(), b"{\"x\":1}");
        assert!(verify(&text, "licence", &keys)
            .unwrap_err()
            .0
            .contains("not \"licence\""));
        assert!(verify(&text, "profile-pack", &[])
            .unwrap_err()
            .0
            .contains("does not trust"));

        let other = sign(&[9; 32], "test", "profile-pack", b"{\"x\":1}");
        assert_eq!(
            verify(&other, "profile-pack", &keys).unwrap_err().0,
            "the signature does not match"
        );
    }

    #[test]
    fn a_changed_payload_or_signature_is_refused() {
        let keys = test_keys();
        let text = sign(&TEST_SEED, "test", "profile-pack", b"{\"x\":1}");
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["payload"] = STANDARD.encode(b"{\"x\":2}").into();
        assert!(verify(&v.to_string(), "profile-pack", &keys).is_err());

        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["signature"] = STANDARD.encode([0u8; 64]).into();
        assert!(verify(&v.to_string(), "profile-pack", &keys).is_err());
        v["signature"] = "short".into();
        assert!(verify(&v.to_string(), "profile-pack", &keys).is_err());

        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["extra"] = 1.into();
        assert!(verify(&v.to_string(), "profile-pack", &keys).is_err());
        assert!(verify("not json", "profile-pack", &keys).is_err());
    }
}
