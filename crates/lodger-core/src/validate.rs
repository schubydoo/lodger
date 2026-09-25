//! Input checks that run before any input reaches `lodger-virt`.
//!
//! `virt` turns every string into a C string with `unwrap()`, so a NUL byte
//! panics. Every string that goes to libvirt must therefore pass through
//! [`Name::parse`] or [`check_text`] first.

use std::fmt;
use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

/// The longest name Lodger accepts for a VM, pool, volume, network, or snapshot.
pub const NAME_MAX_LEN: usize = 64;

/// The longest prefix Lodger accepts for a network subnet. A /30 has 2 usable
/// addresses: the gateway on the host and one VM.
pub const SUBNET_MAX_PREFIX: u8 = 30;

/// The RFC 1918 private ranges. A NAT or isolated network outside them would
/// hide real hosts on the internet from every VM.
const PRIVATE_RANGES: [Ipv4Net; 3] = [
    Ipv4Net::new_assert(Ipv4Addr::new(10, 0, 0, 0), 8),
    Ipv4Net::new_assert(Ipv4Addr::new(172, 16, 0, 0), 12),
    Ipv4Net::new_assert(Ipv4Addr::new(192, 168, 0, 0), 16),
];

/// Why a piece of input was rejected. `field` names the input, for example
/// "VM name", so the message tells the user what to fix.
///
/// The messages never repeat the whole input: it can be long or hostile.
/// A rejected character is shown with Rust escapes, so control characters
/// cannot reach a log or terminal raw.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    #[error("{field} contains a NUL byte")]
    Nul { field: &'static str },
    #[error("{field} is empty")]
    Empty { field: &'static str },
    #[error("{field} has {len} characters, but the limit is {max}")]
    TooLong {
        field: &'static str,
        len: usize,
        max: usize,
    },
    #[error("{field} must start with a letter or a digit, not {ch:?}")]
    BadFirstChar { field: &'static str, ch: char },
    #[error("{field} contains {ch:?}. Use only letters, digits, '.', '_', and '-'")]
    BadChar { field: &'static str, ch: char },
    #[error("{field} is not an IPv4 subnet such as 192.168.150.0/24")]
    SubnetSyntax { field: &'static str },
    #[error("{field} has host bits set. Use {network}")]
    SubnetHostBits {
        field: &'static str,
        network: Ipv4Net,
    },
    #[error("{field} is not in 10.0.0.0/8, 172.16.0.0/12, or 192.168.0.0/16")]
    SubnetNotPrivate { field: &'static str },
    #[error("{field} is too small. The prefix must be /{max} or shorter")]
    SubnetTooSmall { field: &'static str, max: u8 },
    #[error("{field} overlaps {other_subnet}, which network {other_name:?} uses")]
    SubnetOverlap {
        field: &'static str,
        other_name: String,
        other_subnet: Ipv4Net,
    },
    #[error("{field} must be an absolute path that starts with /")]
    PathNotAbsolute { field: &'static str },
    #[error("{field} must not contain . or .. as a folder name")]
    PathDotSegment { field: &'static str },
    #[error("{field} must not contain a control character")]
    PathControl { field: &'static str },
    #[error(
        "{field} {path} is a system folder or is inside one. Use a folder of its own, such as /var/lib/libvirt/images"
    )]
    SystemPath { field: &'static str, path: String },
    #[error("{field} is not a host name or an IP address")]
    HostSyntax { field: &'static str },
    #[error("{field} is the folder of pool {other_name:?} already")]
    PathInUse {
        field: &'static str,
        other_name: String,
    },
    #[error("pool {other_name:?} mounts this NFS export already")]
    NfsExportInUse { other_name: String },
    #[error("{field} must be 1 to 15 letters, digits, '.', '_', or '-'")]
    InterfaceName { field: &'static str },
    #[error(
        "there is no bridge {name:?} on this host. A host bridge must exist first: create it on the host, for example with NetworkManager or systemd-networkd, then pick it here"
    )]
    NoSuchBridge { name: String },
    #[error("{field} must be between {min} and {max} bytes")]
    SizeOutOfRange {
        field: &'static str,
        min: u64,
        max: u64,
    },
    #[error("pool {pool:?} has a volume {name:?} already")]
    VolumeNameInUse { pool: String, name: String },
}

/// The longest Linux interface name (`IFNAMSIZ` minus the NUL byte).
pub const INTERFACE_MAX_LEN: usize = 15;

/// Checks a Linux network interface name, such as a host bridge `br0`.
pub fn parse_interface(field: &'static str, value: &str) -> Result<String, InputError> {
    check_text(field, value)?;
    let fine = !value.is_empty()
        && value.len() <= INTERFACE_MAX_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !matches!(value, "." | "..");
    if fine {
        Ok(value.to_owned())
    } else {
        Err(InputError::InterfaceName { field })
    }
}

/// The longest path Lodger accepts, as Linux `PATH_MAX`.
pub const PATH_MAX_LEN: usize = 4096;

/// Checks an absolute path and returns it in its plain form: one slash
/// between folders and no slash at the end. `.` and `..` are rejected, so the
/// plain form names the same place as the input.
pub fn parse_path(field: &'static str, value: &str) -> Result<String, InputError> {
    check_text(field, value)?;
    if value.is_empty() {
        return Err(InputError::Empty { field });
    }
    if !value.starts_with('/') {
        return Err(InputError::PathNotAbsolute { field });
    }
    if value.len() > PATH_MAX_LEN {
        return Err(InputError::TooLong {
            field,
            len: value.len(),
            max: PATH_MAX_LEN,
        });
    }
    let parts: Vec<&str> = value.split('/').filter(|p| !p.is_empty()).collect();
    if parts.iter().any(|p| matches!(*p, "." | "..")) {
        return Err(InputError::PathDotSegment { field });
    }
    // XML 1.0 cannot carry these, so a pool with such a path would get XML
    // that libvirt cannot parse. The fuzzer found it.
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{FFFE}' | '\u{FFFF}'))
    {
        return Err(InputError::PathControl { field });
    }
    Ok(format!("/{}", parts.join("/")))
}

/// Checks a host name, an IPv4 address, or an IPv6 address, such as an NFS
/// server. The allowed characters keep it safe in a mount command line.
pub fn parse_host(field: &'static str, value: &str) -> Result<String, InputError> {
    check_text(field, value)?;
    if value.is_empty() {
        return Err(InputError::Empty { field });
    }
    // 253 is the longest DNS name.
    if value.len() > 253 {
        return Err(InputError::TooLong {
            field,
            len: value.len(),
            max: 253,
        });
    }
    let fine = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    if !fine || value.starts_with(['-', '.']) {
        return Err(InputError::HostSyntax { field });
    }
    Ok(value.to_owned())
}

/// Rejects free text that contains a NUL byte, for example an SSH key or a
/// path. Use [`Name::parse`] for names.
pub fn check_text<'a>(field: &'static str, value: &'a str) -> Result<&'a str, InputError> {
    if value.contains('\0') {
        return Err(InputError::Nul { field });
    }
    Ok(value)
}

/// A name that Lodger may give to a new VM, pool, volume, network, or
/// snapshot: 1 to 64 ASCII letters, digits, `.`, `_`, or `-`, starting with a
/// letter or a digit.
///
/// libvirt itself accepts almost any name. Lodger accepts fewer, so a name is
/// also safe as a file name in a pool directory and in a shell argument list.
/// Objects that other tools created keep their names: model types hold those
/// as plain strings, and API paths use UUIDs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Name(String);

impl Name {
    /// Checks `value` against the allowlist.
    pub fn parse(field: &'static str, value: &str) -> Result<Self, InputError> {
        check_text(field, value)?;
        let mut chars = value.chars();
        let Some(first) = chars.next() else {
            return Err(InputError::Empty { field });
        };
        if !first.is_ascii_alphanumeric() {
            return Err(InputError::BadFirstChar { field, ch: first });
        }
        if let Some(ch) =
            chars.find(|&c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        {
            return Err(InputError::BadChar { field, ch });
        }
        // Every character is ASCII here, so the byte length is the character count.
        if value.len() > NAME_MAX_LEN {
            return Err(InputError::TooLong {
                field,
                len: value.len(),
                max: NAME_MAX_LEN,
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Parses a subnet for a NAT or isolated network, for example
/// `192.168.150.0/24`. The subnet must be private, must have no host bits
/// set, and must leave room for the gateway and at least one VM.
pub fn parse_subnet(field: &'static str, value: &str) -> Result<Ipv4Net, InputError> {
    check_text(field, value)?;
    let net: Ipv4Net = value
        .parse()
        .map_err(|_| InputError::SubnetSyntax { field })?;
    // ipnet reads "010" as 10, but other tools read it as octal 8. Only the
    // canonical form is unambiguous.
    if net.to_string() != value {
        return Err(InputError::SubnetSyntax { field });
    }
    if !PRIVATE_RANGES.iter().any(|range| range.contains(&net)) {
        return Err(InputError::SubnetNotPrivate { field });
    }
    if net.prefix_len() > SUBNET_MAX_PREFIX {
        return Err(InputError::SubnetTooSmall {
            field,
            max: SUBNET_MAX_PREFIX,
        });
    }
    // Last, so the suggested network passes every other check.
    if net.addr() != net.network() {
        return Err(InputError::SubnetHostBits {
            field,
            network: net.trunc(),
        });
    }
    Ok(net)
}

/// Rejects `subnet` if it overlaps a subnet in `existing`, and names the
/// first network it overlaps.
pub fn check_no_overlap<'a>(
    field: &'static str,
    subnet: Ipv4Net,
    existing: impl IntoIterator<Item = (&'a str, Ipv4Net)>,
) -> Result<(), InputError> {
    match existing
        .into_iter()
        .find(|(_, other)| subnet.contains(&other.network()) || other.contains(&subnet.network()))
    {
        Some((name, other)) => Err(InputError::SubnetOverlap {
            field,
            other_name: name.to_owned(),
            other_subnet: other,
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: &str = "VM name";

    fn net(s: &str) -> Ipv4Net {
        s.parse().unwrap()
    }

    #[test]
    fn valid_names_pass() {
        for name in [
            "a",
            "web01",
            "db-primary",
            "ubuntu_24.04",
            "Z9",
            &"x".repeat(NAME_MAX_LEN),
        ] {
            assert_eq!(Name::parse(F, name).unwrap().as_str(), name);
        }
    }

    #[test]
    fn empty_name_fails() {
        assert_eq!(Name::parse(F, ""), Err(InputError::Empty { field: F }));
    }

    #[test]
    fn too_long_name_fails() {
        let long = "x".repeat(NAME_MAX_LEN + 1);
        assert_eq!(
            Name::parse(F, &long),
            Err(InputError::TooLong {
                field: F,
                len: NAME_MAX_LEN + 1,
                max: NAME_MAX_LEN
            })
        );
    }

    #[test]
    fn nul_fails_before_any_other_check() {
        for value in [
            "\0",
            "web\0",
            "\0web",
            "a\0b",
            &format!("{}\0", "x".repeat(100)),
        ] {
            assert_eq!(
                Name::parse(F, value),
                Err(InputError::Nul { field: F }),
                "{value:?}"
            );
            assert_eq!(check_text(F, value), Err(InputError::Nul { field: F }));
            assert_eq!(parse_subnet(F, value), Err(InputError::Nul { field: F }));
        }
    }

    #[test]
    fn names_must_start_with_a_letter_or_digit() {
        for (value, ch) in [
            (".hidden", '.'),
            ("-rf", '-'),
            ("_x", '_'),
            ("..", '.'),
            (" web", ' '),
        ] {
            assert_eq!(
                Name::parse(F, value),
                Err(InputError::BadFirstChar { field: F, ch })
            );
        }
    }

    #[test]
    fn hostile_names_fail() {
        for (value, ch) in [
            ("a/../etc", '/'),
            ("a\\b", '\\'),
            ("web 01", ' '),
            ("web\n01", '\n'),
            ("x;rm", ';'),
            ("a$(id)", '$'),
            ("a`id`", '`'),
            ("a<b", '<'),
            ("a'b", '\''),
            ("a\"b", '"'),
            ("web\u{1b}[2J", '\u{1b}'),
            ("wеb", 'е'), // Cyrillic е, a lookalike of Latin e.
            ("web\u{200b}", '\u{200b}'),
        ] {
            assert_eq!(
                Name::parse(F, value),
                Err(InputError::BadChar { field: F, ch }),
                "{value:?}"
            );
        }
    }

    #[test]
    fn a_hostile_character_wins_over_length() {
        let value = format!("{}/", "x".repeat(1000));
        assert_eq!(
            Name::parse(F, &value),
            Err(InputError::BadChar { field: F, ch: '/' })
        );
    }

    #[test]
    fn messages_name_the_field_and_escape_control_characters() {
        let msg = Name::parse(F, "web\u{1b}x").unwrap_err().to_string();
        assert_eq!(
            msg,
            "VM name contains '\\u{1b}'. Use only letters, digits, '.', '_', and '-'"
        );
        assert!(!msg.contains('\u{1b}'));
        assert_eq!(
            Name::parse(F, &"x".repeat(65)).unwrap_err().to_string(),
            "VM name has 65 characters, but the limit is 64"
        );
    }

    #[test]
    fn name_displays_as_itself() {
        let name = Name::parse(F, "web01").unwrap();
        assert_eq!(name.to_string(), "web01");
        assert_eq!(name.as_ref(), "web01");
    }

    #[test]
    fn free_text_passes_without_nul() {
        let key = "ssh-ed25519 AAAAC3Nz user@host";
        assert_eq!(check_text("SSH key", key), Ok(key));
        assert_eq!(check_text("SSH key", ""), Ok(""));
    }

    #[test]
    fn valid_subnets_pass() {
        for s in [
            "192.168.150.0/24",
            "10.0.0.0/8",
            "172.16.0.0/12",
            "172.31.255.252/30",
            "10.20.0.0/16",
        ] {
            assert_eq!(parse_subnet("subnet", s), Ok(net(s)));
        }
    }

    #[test]
    fn malformed_subnets_fail() {
        for s in [
            "",
            "192.168.150.0",
            "192.168.150.0/33",
            "192.168.150/24",
            "192.168.1.010/24",
            " 10.0.0.0/8",
            "fd00::/64",
            "web",
        ] {
            assert_eq!(
                parse_subnet("subnet", s),
                Err(InputError::SubnetSyntax { field: "subnet" }),
                "{s:?}"
            );
        }
    }

    #[test]
    fn subnet_with_host_bits_names_the_network() {
        let err = parse_subnet("subnet", "192.168.150.1/24").unwrap_err();
        assert_eq!(
            err,
            InputError::SubnetHostBits {
                field: "subnet",
                network: net("192.168.150.0/24")
            }
        );
        assert_eq!(
            err.to_string(),
            "subnet has host bits set. Use 192.168.150.0/24"
        );
    }

    #[test]
    fn host_bits_error_comes_last_so_its_suggestion_passes() {
        assert_eq!(
            parse_subnet("subnet", "8.8.8.1/24"),
            Err(InputError::SubnetNotPrivate { field: "subnet" })
        );
        assert_eq!(
            parse_subnet("subnet", "10.0.0.1/31"),
            Err(InputError::SubnetTooSmall {
                field: "subnet",
                max: 30
            })
        );
    }

    #[test]
    fn public_subnets_fail() {
        for s in [
            "8.8.8.0/24",
            "172.32.0.0/16",
            "172.0.0.0/8",
            "192.0.0.0/8",
            "0.0.0.0/0",
            "127.0.0.0/8",
        ] {
            assert_eq!(
                parse_subnet("subnet", s),
                Err(InputError::SubnetNotPrivate { field: "subnet" }),
                "{s:?}"
            );
        }
    }

    #[test]
    fn subnets_smaller_than_a_30_fail() {
        for s in ["10.0.0.0/31", "10.0.0.0/32"] {
            assert_eq!(
                parse_subnet("subnet", s),
                Err(InputError::SubnetTooSmall {
                    field: "subnet",
                    max: 30
                })
            );
        }
    }

    #[test]
    fn overlapping_subnets_fail_and_name_the_network() {
        let existing = [
            ("default", net("192.168.122.0/24")),
            ("lab", net("10.10.0.0/16")),
        ];
        for s in [
            "192.168.122.0/24",
            "192.168.122.128/25",
            "192.168.0.0/16",
            "10.10.5.0/24",
            "10.0.0.0/8",
        ] {
            let err = check_no_overlap("subnet", net(s), existing).unwrap_err();
            assert!(matches!(err, InputError::SubnetOverlap { .. }), "{s}");
        }
        assert_eq!(
            check_no_overlap("subnet", net("192.168.122.0/25"), existing)
                .unwrap_err()
                .to_string(),
            "subnet overlaps 192.168.122.0/24, which network \"default\" uses"
        );
    }

    #[test]
    fn disjoint_subnets_pass() {
        let existing = [("default", net("192.168.122.0/24"))];
        for s in ["192.168.123.0/24", "192.168.121.0/24", "10.0.0.0/8"] {
            assert_eq!(check_no_overlap("subnet", net(s), existing), Ok(()));
        }
        assert_eq!(check_no_overlap("subnet", net("10.0.0.0/8"), []), Ok(()));
    }

    #[test]
    fn a_path_comes_back_in_its_plain_form() {
        assert_eq!(parse_path("Path", "/srv//vm/").unwrap(), "/srv/vm");
        assert_eq!(parse_path("Path", "/").unwrap(), "/");
        assert_eq!(
            parse_path("Path", ""),
            Err(InputError::Empty { field: "Path" })
        );
        assert_eq!(
            parse_path("Path", "srv"),
            Err(InputError::PathNotAbsolute { field: "Path" })
        );
        assert_eq!(
            parse_path("Path", "/a/../b"),
            Err(InputError::PathDotSegment { field: "Path" })
        );
        // XML cannot carry a control character or U+FFFE/U+FFFF.
        for path in [
            "/\u{1}",
            "/srv/a\nb",
            "/srv/\u{7f}",
            "/srv/\u{FFFE}",
            "/srv/\u{85}",
        ] {
            assert_eq!(
                parse_path("Path", path),
                Err(InputError::PathControl { field: "Path" }),
                "{path:?}"
            );
        }
        // A name that only starts with dots is a normal folder name.
        assert_eq!(parse_path("Path", "/a/..b/.c").unwrap(), "/a/..b/.c");
        let long = format!("/{}", "a".repeat(PATH_MAX_LEN));
        assert!(matches!(
            parse_path("Path", &long),
            Err(InputError::TooLong { .. })
        ));
    }

    #[test]
    fn hosts_allow_names_and_addresses_only() {
        for host in ["nas", "nas.lan", "10.0.0.5", "fd00::5", "a-b.example.com"] {
            assert_eq!(parse_host("Host", host).unwrap(), host);
        }
        for host in ["", "-o", ".lan", "nas lan", "nas/x", "nas;x", "n\0as"] {
            assert!(parse_host("Host", host).is_err(), "{host}");
        }
        assert!(matches!(
            parse_host("Host", &"a".repeat(254)),
            Err(InputError::TooLong { .. })
        ));
    }
}
