//! Argon2id password hashing for user credentials (PHC string format).

use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};

use pdm_core::PdmError;

/// Hash a password with Argon2id (crate default parameters) and a random
/// 16-byte salt, returning the PHC string (`$argon2id$v=19$...`).
pub fn hash_password(password: &str) -> Result<String, PdmError> {
    let salt = SaltString::generate(OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| PdmError::Internal(format!("password hash failed: {e}")))?;
    Ok(hash.to_string())
}

/// Verify a password against a stored PHC hash.
///
/// `Ok(true)` / `Ok(false)` for match / mismatch, `Err` when the stored hash
/// is malformed or not an Argon2 hash.
pub fn verify_password(password: &str, stored_hash: &str) -> Result<bool, PdmError> {
    let parsed = PasswordHash::new(stored_hash)
        .map_err(|e| PdmError::Internal(format!("malformed stored password hash: {e}")))?;
    match Argon2::default().verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(e) => Err(PdmError::Internal(format!("password verify failed: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_verify_roundtrip() -> Result<(), PdmError> {
        let hash = hash_password("s3kr3t-pa$$")?;
        assert!(hash.starts_with("$argon2id$v=19$"));
        assert!(verify_password("s3kr3t-pa$$", &hash)?);
        Ok(())
    }

    #[test]
    fn wrong_password_fails() -> Result<(), PdmError> {
        let hash = hash_password("correct horse battery staple")?;
        assert!(!verify_password("wrong password", &hash)?);
        Ok(())
    }

    #[test]
    fn verify_rejects_malformed_hash() {
        assert!(verify_password("pw", "not-a-phc-string").is_err());
        assert!(verify_password("pw", "").is_err());
        assert!(verify_password("pw", "$argon2id$").is_err());
    }
}
