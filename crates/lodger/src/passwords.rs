//! Password hashes: argon2id at the OWASP minimum, which the `argon2` 0.6.0
//! defaults are: 19456 KiB, 2 iterations, 1 lane (TAD section 7.1).
//!
//! Both functions take tens of milliseconds and 19 MiB, so callers run them
//! with `spawn_blocking`.

use std::sync::LazyLock;

use argon2::password_hash::{PasswordHasher, PasswordVerifier};

/// Hashes a new password into a PHC string with a fresh salt.
pub fn hash(password: &str) -> Result<String, String> {
    argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| e.to_string())
}

/// Checks a password against a stored PHC string.
pub fn verify(password: &str, stored: &str) -> bool {
    argon2::Argon2::default()
        .verify_password(password.as_bytes(), stored)
        .is_ok()
}

/// A hash of a random password, to verify against when the username does
/// not exist. The login then takes as long as for a real account, so its
/// timing does not tell which usernames exist.
static DUMMY: LazyLock<String> = LazyLock::new(|| {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS gives random bytes");
    hash(&hex::encode(bytes)).expect("argon2 hashes a fixed-size input")
});

/// Spends the same time as [`verify`], and always fails.
pub fn verify_nobody(password: &str) -> bool {
    let _ = verify(password, &DUMMY);
    false
}

#[cfg(test)]
mod tests {
    use super::{hash, verify, verify_nobody};

    #[test]
    fn a_hash_verifies_only_its_password() {
        let stored = hash("correct horse battery staple").unwrap();
        assert!(
            stored.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{stored}"
        );
        assert!(verify("correct horse battery staple", &stored));
        assert!(!verify("correct horse battery stable", &stored));
        assert!(!verify("correct horse battery staple", "not a hash"));
    }

    #[test]
    fn each_hash_has_its_own_salt() {
        assert_ne!(
            hash("same password here!").unwrap(),
            hash("same password here!").unwrap()
        );
    }

    #[test]
    fn nobody_never_verifies() {
        assert!(!verify_nobody("anything at all"));
    }
}
