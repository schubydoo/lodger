//! The VNC display of a domain, reached through a socket pair, so Lodger
//! needs no VNC port on the host (TAD section 4.3). A VM whose XML gives
//! VNC a `listen` address still opens its own port in QEMU; only VMs with
//! `<listen type='none'/>` have none.
//!
//! Lodger makes the pair itself and hands one end to libvirt with
//! `virDomainOpenGraphics`. libvirt passes a copy to QEMU, and Lodger keeps
//! the other end. `virDomainOpenGraphicsFD` does the same inside libvirt,
//! but it returns a raw file descriptor, and safe Rust cannot take
//! ownership of one. This way stays in safe Rust.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use uuid::Uuid;
use virt::sys::VIR_DOMAIN_OPEN_GRAPHICS_SKIPAUTH;

use crate::conn::{Error, Virt};

impl Virt {
    /// Opens the first graphics device of domain `id` and returns Lodger's
    /// end of the socket, which speaks RFB (the VNC protocol).
    ///
    /// Lodger checks the user itself, so the VM's own VNC password is
    /// skipped. The socket closes when the returned stream drops.
    pub async fn open_vnc(&self, id: Uuid) -> Result<UnixStream, Error> {
        let (ours, theirs) = UnixStream::pair()?;
        self.read(move |c| {
            let domain = c.lookup_domain_by_uuid(id)?;
            // libvirt sends a copy of the descriptor with the call, so
            // `theirs` can close when the closure ends.
            domain.open_graphics(0, theirs.as_raw_fd(), VIR_DOMAIN_OPEN_GRAPHICS_SKIPAUTH)
        })
        .await?;
        Ok(ours)
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::Virt;

    const TEST_URI: &str = "test:///default";

    #[tokio::test]
    async fn an_unknown_domain_is_not_found() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let err = virt.open_vnc(Uuid::from_u128(0xdead)).await.unwrap_err();
        assert!(err.is_not_found(), "{err}");
    }

    #[tokio::test]
    async fn the_test_driver_has_no_display() {
        // The test driver cannot open graphics, so this proves only that
        // the error comes back as an error. The host test covers a real VM.
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt
            .read(|c| c.lookup_domain_by_name("test")?.uuid())
            .await
            .unwrap();
        let err = virt.open_vnc(id).await.unwrap_err();
        assert!(!err.is_not_found(), "{err}");
    }
}
