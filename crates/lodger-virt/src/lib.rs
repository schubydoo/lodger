//! The libvirt adapter. This is the only crate that imports `virt`.

mod cache;
mod conn;
mod console;
mod events;
mod supervisor;

pub use cache::Inventory;
pub use conn::{Error, Virt};
pub use events::{DomainChange, Event};
pub use supervisor::{ConnState, Host};

/// Returns the version of the libvirt client library that Lodger links to,
/// as `(major, minor, micro)`.
pub fn client_library_version() -> Result<(u32, u32, u32), virt::error::Error> {
    let v = virt::connect::Connect::version()?;
    Ok((v / 1_000_000, v / 1000 % 1000, v % 1000))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_the_client_library_version() {
        let (major, _, _) = super::client_library_version().unwrap();
        assert!(major >= 9, "Lodger needs libvirt 9.0.0 or newer");
    }
}
