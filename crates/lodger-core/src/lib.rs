//! Pure logic for Lodger: model types, XML builders and editors, seed disks,
//! input checks, and the password policy. This crate has no system dependencies, so its tests run
//! with only the Rust toolchain.

pub mod model;
pub mod password;
pub mod validate;
