//! Ed25519 update signatures, as Sparkle and WinSparkle use them: a raw
//! signature over the whole downloaded file, base64 in the appcast's
//! `sparkle:edSignature`, checked against a base64 32-byte public key.
//! `openssl pkeyutl -sign -rawin` produces the same thing.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};

pub fn public_key(base64_key: &str) -> Result<VerifyingKey, String> {
    let bytes = STANDARD
        .decode(base64_key.trim())
        .map_err(|e| format!("the public key isn't valid base64: {e}"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|b: Vec<u8>| format!("the public key is {} bytes, not 32", b.len()))?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| format!("the public key isn't a valid Ed25519 key: {e}"))
}

pub fn check(key: &VerifyingKey, base64_signature: &str, data: &[u8]) -> Result<(), String> {
    let sig = STANDARD
        .decode(base64_signature.trim())
        .map_err(|e| format!("the update's signature isn't valid base64: {e}"))?;
    let sig: [u8; 64] = sig
        .try_into()
        .map_err(|s: Vec<u8>| format!("the update's signature is {} bytes, not 64", s.len()))?;
    key.verify_strict(data, &Signature::from_bytes(&sig))
        .map_err(|_| "the update's signature doesn't match; it may have been tampered with".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with openssl, the way a release pipeline signs:
    //   openssl genpkey -algorithm ed25519 -out key.pem
    //   openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64
    //   printf 'hello update' > f
    //   openssl pkeyutl -sign -inkey key.pem -rawin -in f | base64
    const KEY: &str = include_str!("../tests/data/openssl_pub.b64");
    const SIG: &str = include_str!("../tests/data/openssl_sig.b64");

    #[test]
    fn accepts_openssl_signatures() {
        let key = public_key(KEY).unwrap();
        check(&key, SIG, b"hello update").unwrap();
        assert!(check(&key, SIG, b"hello updatf").is_err());
    }

    #[test]
    fn rejects_bad_keys() {
        assert!(public_key("not base64!").is_err());
        assert!(public_key("AAAA").is_err());
    }
}
