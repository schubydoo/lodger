//! The input checks in `lodger_core::validate` and the password policy.
//!
//! Each check must return an error, never panic. What a check accepts must
//! keep the promise in its documentation, and checking the accepted value a
//! second time must give the same value.
#![no_main]

use libfuzzer_sys::fuzz_target;
use lodger_core::password;
use lodger_core::validate::{Name, check_text, parse_host, parse_interface, parse_path, parse_subnet};

fuzz_target!(|value: &str| {
    if let Ok(name) = Name::parse("Name", value) {
        let s = name.as_str();
        assert!((1..=64).contains(&s.len()), "{s:?}");
        assert!(s.starts_with(|c: char| c.is_ascii_alphanumeric()), "{s:?}");
        assert!(
            s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "{s:?}"
        );
    }

    if let Ok(path) = parse_path("Path", value) {
        assert!(path.starts_with('/'), "{path:?}");
        assert!(!path.chars().any(char::is_control), "{path:?}");
        assert!(path == "/" || !path.ends_with('/'), "{path:?}");
        assert!(!path.contains("//"), "{path:?}");
        assert!(path.split('/').all(|p| p != "." && p != ".."), "{path:?}");
        assert_eq!(parse_path("Path", &path).as_deref(), Ok(path.as_str()));
    }

    if let Ok(host) = parse_host("Host", value) {
        assert!(!host.is_empty() && host.len() <= 253, "{host:?}");
        assert!(!host.starts_with(['-', '.']), "{host:?}");
        assert!(
            host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')),
            "{host:?}"
        );
    }

    if let Ok(interface) = parse_interface("Bridge", value) {
        assert_eq!(parse_interface("Bridge", &interface).as_deref(), Ok(interface.as_str()));
    }

    if let Ok(subnet) = parse_subnet("Subnet", value) {
        assert!(subnet.prefix_len() <= 30, "{subnet}");
        assert_eq!(parse_subnet("Subnet", &subnet.to_string()), Ok(subnet));
    }

    if check_text("Text", value).is_ok() {
        assert!(!value.contains('\0'));
    }

    let _ = password::check(value);
});
