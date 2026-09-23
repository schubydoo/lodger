//! The password policy (TAD section 7.1, OWASP ASVS 5.0 chapter 6.2).
//!
//! - At least [`MIN_CHARS`] characters. Characters, not bytes: "ü" counts as
//!   one (ASVS 6.2.1).
//! - At most [`MAX_CHARS`], so a huge input cannot waste the server's time.
//!   ASVS 6.2.9 asks to allow at least 64.
//! - Not one of the 3000 most common passwords that meet the length rule
//!   (ASVS 6.2.4), compared without regard to case.
//! - No composition rules such as "one digit", which ASVS 6.2.5 discourages.
//!
//! `common.txt` holds the first 3000 entries of 15 or more printable ASCII
//! characters, in frequency order, from `SecLists`
//! `Passwords/Common-Credentials/Pwdb_top-1000000.txt` at commit
//! 913b327317496d062bcc7cace524aaad8a693be2 (MIT). The top 3000 of the full
//! list contain no password of 15 characters, so they would reject nothing
//! that the length rule lets through.

use std::collections::HashSet;
use std::sync::LazyLock;

pub const MIN_CHARS: usize = 15;
pub const MAX_CHARS: usize = 1024;

const COMMON: &str = include_str!("common.txt");

static COMMON_SET: LazyLock<HashSet<String>> =
    LazyLock::new(|| COMMON.lines().map(str::to_lowercase).collect());

/// Why a new password was refused. The messages never repeat the password.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    #[error("the password has {len} characters, but it needs at least {MIN_CHARS}")]
    TooShort { len: usize },
    #[error("the password has {len} characters, but the limit is {MAX_CHARS}")]
    TooLong { len: usize },
    #[error("the password is on the list of the most common passwords. Choose another one")]
    Common,
}

/// Checks a new password against the policy.
pub fn check(password: &str) -> Result<(), PasswordError> {
    let len = password.chars().count();
    if len < MIN_CHARS {
        return Err(PasswordError::TooShort { len });
    }
    if len > MAX_CHARS {
        return Err(PasswordError::TooLong { len });
    }
    if COMMON_SET.contains(&password.to_lowercase()) {
        return Err(PasswordError::Common);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{COMMON, MAX_CHARS, MIN_CHARS, PasswordError, check};

    #[test]
    fn the_list_has_3000_entries_that_meet_the_length_rule() {
        let entries: Vec<&str> = COMMON.lines().collect();
        assert_eq!(entries.len(), 3000);
        for entry in entries {
            assert!(entry.chars().count() >= MIN_CHARS, "{entry:?} is too short");
            assert!(
                entry.bytes().all(|b| (0x20..0x7f).contains(&b)),
                "{entry:?}"
            );
        }
    }

    #[test]
    fn a_long_uncommon_password_passes() {
        assert_eq!(check("correct horse battery staple"), Ok(()));
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // 15 characters, 30 bytes.
        assert_eq!(check("üüüüüüüüüüüüüüü"), Ok(()));
        assert_eq!(
            check("üüüüüüüüüüüüüü"),
            Err(PasswordError::TooShort { len: 14 })
        );
    }

    #[test]
    fn short_and_long_passwords_fail() {
        assert_eq!(check(""), Err(PasswordError::TooShort { len: 0 }));
        let long = "x".repeat(MAX_CHARS + 1);
        assert_eq!(
            check(&long),
            Err(PasswordError::TooLong { len: MAX_CHARS + 1 })
        );
        assert_eq!(check(&"xy".repeat(MAX_CHARS / 2)), Ok(()));
    }

    #[test]
    fn common_passwords_fail_in_any_case() {
        let first = COMMON.lines().next().unwrap();
        assert_eq!(check(first), Err(PasswordError::Common));
        assert_eq!(check(&first.to_uppercase()), Err(PasswordError::Common));
        assert_eq!(check("1q2w3e4r5t6y7u8i"), Err(PasswordError::Common));
    }

    #[test]
    fn a_message_never_repeats_the_password() {
        let err = check("1q2w3e4r5t6y7u8i").unwrap_err().to_string();
        assert!(!err.contains("1q2w3e4r5t6y7u8i"), "{err}");
    }
}
