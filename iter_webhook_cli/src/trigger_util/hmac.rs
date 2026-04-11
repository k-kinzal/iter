//! GitHub-style HMAC-SHA256 signature verification.
//!
//! GitHub sends webhook signatures in the `X-Hub-Signature-256` header,
//! formatted as `sha256=<hex>` where `<hex>` is the lowercase hex encoding of
//! the HMAC-SHA256 of the raw request body keyed by a shared secret.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Verify a `sha256=<hex>` signature against `body` using `secret`.
///
/// Returns `true` only when the header parses, the hex decodes, and the MAC
/// verifies. Constant-time comparison is delegated to [`hmac::Mac::verify_slice`].
#[must_use]
pub fn verify_github_signature(secret: &[u8], header: &str, body: &[u8]) -> bool {
    let Some(hex_part) = header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(provided) = hex::decode(hex_part.trim()) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&provided).is_ok()
}

/// Compute a `sha256=<hex>` signature for `body` using `secret`.
///
/// Primarily intended for tests; production code calls
/// [`verify_github_signature`] on the receive side.
///
/// # Panics
///
/// Never. HMAC-SHA256 accepts any key length, so `new_from_slice` cannot
/// fail; the `expect` is unreachable.
#[cfg(test)]
#[must_use]
pub fn sign_github(secret: &[u8], body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_then_verify_succeeds() {
        let secret = b"swordfish";
        let body = br#"{"hello":"world"}"#;
        let sig = sign_github(secret, body);
        assert!(verify_github_signature(secret, &sig, body));
    }

    #[test]
    fn wrong_secret_fails() {
        let body = br#"{"hello":"world"}"#;
        let sig = sign_github(b"swordfish", body);
        assert!(!verify_github_signature(b"other", &sig, body));
    }

    #[test]
    fn malformed_header_fails() {
        assert!(!verify_github_signature(b"k", "not-a-sig", b"body"));
        assert!(!verify_github_signature(b"k", "sha256=zz", b"body"));
    }

    #[test]
    fn body_tamper_fails() {
        let secret = b"swordfish";
        let body = br#"{"hello":"world"}"#;
        let sig = sign_github(secret, body);
        assert!(!verify_github_signature(secret, &sig, b"different"));
    }
}
