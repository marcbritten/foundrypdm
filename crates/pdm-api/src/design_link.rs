//! Short-lived signed links for `GET /public/design/{version_id}/{filename}`.
//!
//! The bundled SolidCraft web editor fetches the design it opens without credentials, so that
//! route skips the bearer token. Instead, a signed-in caller asks
//! `POST /versions/{version_id}/design-link`, which checks the organization and returns the link
//! with an expiry and an HMAC-SHA256 signature over the version, the file name and the expiry.
//! The public route serves the bytes only while the signature is valid and the link has not
//! expired, so a link that leaves the organization (a pasted URL, a log line, browser history)
//! stops working after [`TTL_SECS`].
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// How long a design link stays valid. The editor fetches it as it opens, and its
/// "Download original" button asks for a fresh link.
pub const TTL_SECS: i64 = 600;

/// Why a design link was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    /// Missing, malformed or wrong signature (wrong key, version, name or expiry).
    Invalid,
    /// A valid signature whose expiry has passed.
    Expired,
}

/// The server's design-link key, ready to sign and verify. Cloning is cheap.
#[derive(Clone)]
pub struct DesignLinks {
    mac: HmacSha256,
}

impl DesignLinks {
    /// A signer for `key` (the server secret; see `pdm_db::server_secret`).
    pub fn new(key: &[u8]) -> Result<Self, pdm_core::PdmError> {
        if key.len() < 32 {
            return Err(pdm_core::PdmError::Internal(
                "design-link key must be at least 32 bytes".into(),
            ));
        }
        let mac = HmacSha256::new_from_slice(key)
            .map_err(|e| pdm_core::PdmError::Internal(format!("design-link key: {e}")))?;
        Ok(Self { mac })
    }

    fn mac_for(&self, version_id: Uuid, filename: &str, expires: i64) -> HmacSha256 {
        let mut mac = self.mac.clone();
        mac.update(b"pdm-design-link-v1\0");
        mac.update(version_id.as_bytes());
        mac.update(&expires.to_be_bytes());
        mac.update(filename.as_bytes());
        mac
    }

    /// Hex signature for a link to `version_id`'s `filename`, valid until `expires`
    /// (Unix seconds).
    pub fn sign(&self, version_id: Uuid, filename: &str, expires: i64) -> String {
        hex::encode(
            self.mac_for(version_id, filename, expires)
                .finalize()
                .into_bytes(),
        )
    }

    /// Check a link's signature (in constant time), then its expiry against `now`.
    pub fn verify(
        &self,
        version_id: Uuid,
        filename: &str,
        expires: i64,
        signature: &str,
        now: i64,
    ) -> Result<(), LinkError> {
        let signature = hex::decode(signature).map_err(|_| LinkError::Invalid)?;
        self.mac_for(version_id, filename, expires)
            .verify_slice(&signature)
            .map_err(|_| LinkError::Invalid)?;
        if now >= expires {
            return Err(LinkError::Expired);
        }
        Ok(())
    }
}

/// Percent-encode one URL path segment (everything but RFC 3986 unreserved characters).
pub fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{DesignLinks, LinkError, encode_path_segment};
    use pdm_core::PdmError;
    use uuid::Uuid;

    const KEY: [u8; 32] = [7; 32];

    #[test]
    fn a_signed_link_verifies_until_it_expires() -> Result<(), PdmError> {
        let l = DesignLinks::new(&KEY)?;
        let v = Uuid::new_v4();
        let sig = l.sign(v, "pin-v1.sc", 1_000);
        assert_eq!(l.verify(v, "pin-v1.sc", 1_000, &sig, 999), Ok(()));
        assert_eq!(
            l.verify(v, "pin-v1.sc", 1_000, &sig, 1_000),
            Err(LinkError::Expired)
        );
        assert_eq!(
            l.verify(v, "pin-v1.sc", 1_000, &sig, 5_000),
            Err(LinkError::Expired)
        );
        Ok(())
    }

    #[test]
    fn a_changed_version_name_expiry_or_key_is_refused() -> Result<(), PdmError> {
        let l = DesignLinks::new(&KEY)?;
        let v = Uuid::new_v4();
        let sig = l.sign(v, "pin-v1.sc", 1_000);
        let invalid = Err(LinkError::Invalid);
        assert_eq!(
            l.verify(Uuid::new_v4(), "pin-v1.sc", 1_000, &sig, 0),
            invalid
        );
        assert_eq!(l.verify(v, "pin.step", 1_000, &sig, 0), invalid);
        // A later expiry is not accepted either: the signature covers it.
        assert_eq!(l.verify(v, "pin-v1.sc", 9_999, &sig, 0), invalid);
        let other = DesignLinks::new(&[8; 32])?;
        assert_eq!(other.verify(v, "pin-v1.sc", 1_000, &sig, 0), invalid);
        Ok(())
    }

    #[test]
    fn malformed_signatures_are_refused() -> Result<(), PdmError> {
        let l = DesignLinks::new(&KEY)?;
        let v = Uuid::new_v4();
        let sig = l.sign(v, "a.sc", 1_000);
        let longer = format!("{sig}00");
        for bad in ["", "zz", &sig[..sig.len() - 2], longer.as_str()] {
            assert_eq!(l.verify(v, "a.sc", 1_000, bad, 0), Err(LinkError::Invalid));
        }
        // An expired link with a bad signature reports Invalid, not Expired.
        assert_eq!(
            l.verify(v, "a.sc", 1_000, "00", 5_000),
            Err(LinkError::Invalid)
        );
        Ok(())
    }

    #[test]
    fn short_keys_are_refused() {
        assert!(DesignLinks::new(&[1; 16]).is_err());
    }

    #[test]
    fn path_segments_are_percent_encoded() {
        assert_eq!(encode_path_segment("pin-v1.sc"), "pin-v1.sc");
        assert_eq!(
            encode_path_segment("a b/c?d#e%.sc"),
            "a%20b%2Fc%3Fd%23e%25.sc"
        );
        assert_eq!(encode_path_segment("ä.sc"), "%C3%A4.sc");
    }
}
