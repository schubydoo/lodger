//! Start, shut down, and force off (PRD F4, TAD 9.5). Each is one libvirt
//! call. The new state reaches the UI through the lifecycle event, not
//! through the return value.

use uuid::Uuid;
use virt::error::ErrorNumber;
use virt::sys::VIR_DOMAIN_SHUTDOWN_ACPI_POWER_BTN;

use crate::conn::{Error, Virt};

/// A power action on a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Start,
    /// Asks the guest to shut down with the ACPI power button. The domain
    /// stops when the guest finishes.
    Shutdown,
    /// Stops the domain at once, like pulling the power cable.
    ForceOff,
}

impl Power {
    /// The action from its name in the API path.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "start" => Some(Self::Start),
            "shutdown" => Some(Self::Shutdown),
            "force-off" => Some(Self::ForceOff),
            _ => None,
        }
    }

    /// The name in the API path and in the audit log.
    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Shutdown => "shutdown",
            Self::ForceOff => "force-off",
        }
    }
}

impl Virt {
    /// Runs `action` on domain `id`. It runs on the job connection, because
    /// a start can take seconds, and reads must not wait for it.
    ///
    /// The state check comes first, because drivers report a wrong state
    /// with different error codes: QEMU says "operation invalid", and the
    /// test driver says "internal error".
    pub async fn power(&self, id: Uuid, action: Power) -> Result<(), Error> {
        self.job(move |c| {
            let domain = c.lookup_domain_by_uuid(id)?;
            let active = domain.is_active()?;
            if action == Power::Start && active {
                return Ok(Err(Error::WrongState("the VM is running already")));
            }
            if action != Power::Start && !active {
                return Ok(Err(Error::WrongState("the VM is not running")));
            }
            let done = match action {
                Power::Start => domain.create().map(drop),
                Power::Shutdown => {
                    match domain.shutdown_flags(VIR_DOMAIN_SHUTDOWN_ACPI_POWER_BTN) {
                        // A driver without ACPI, such as libvirt's test driver,
                        // rejects the flag: let it pick its own method.
                        Err(e) if e.code().known() == Some(ErrorNumber::InvalidArg) => {
                            domain.shutdown()
                        }
                        other => other,
                    }
                }
                Power::ForceOff => domain.destroy(),
            };
            Ok(done.map_err(Error::from))
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use uuid::Uuid;
    use virt::connect::Connect;
    use virt::sys::{
        VIR_DOMAIN_EVENT_STOPPED, VIR_DOMAIN_EVENT_STOPPED_DESTROYED,
        VIR_DOMAIN_EVENT_STOPPED_SHUTDOWN,
    };

    use super::Power;
    use crate::{DomainChange, Event, Virt};

    const TEST_URI: &str = "test:///default";

    /// Defines a new, shut-off domain. The test driver shares its state
    /// across the process, so each test uses its own name.
    async fn define(virt: &Virt, name: &'static str) -> Uuid {
        virt.read(move |c: &Connect| {
            let xml = format!(
                "<domain type='test'><name>{name}</name><memory>1024</memory>\
                 <os><type>hvm</type></os></domain>"
            );
            c.define_domain_xml(&xml)?.uuid()
        })
        .await
        .unwrap()
    }

    async fn is_active(virt: &Virt, id: Uuid) -> bool {
        virt.read(move |c| c.lookup_domain_by_uuid(id)?.is_active())
            .await
            .unwrap()
    }

    #[test]
    fn names_round_trip_and_unknown_names_fail() {
        for action in [Power::Start, Power::Shutdown, Power::ForceOff] {
            assert_eq!(Power::parse(action.name()), Some(action));
        }
        assert_eq!(Power::parse("reboot"), None);
        assert_eq!(Power::parse("Start"), None);
    }

    #[tokio::test]
    async fn start_shutdown_and_force_off_change_the_state() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = define(&virt, "power-cycle").await;
        assert!(!is_active(&virt, id).await);

        virt.power(id, Power::Start).await.unwrap();
        assert!(is_active(&virt, id).await);
        virt.power(id, Power::Shutdown).await.unwrap();
        assert!(!is_active(&virt, id).await);
        virt.power(id, Power::Start).await.unwrap();
        virt.power(id, Power::ForceOff).await.unwrap();
        assert!(!is_active(&virt, id).await);
    }

    /// Runs `action` and returns the detail code of the domain's next
    /// "stopped" event.
    async fn stop_detail(virt: &Virt, id: Uuid, action: Power) -> i32 {
        let mut events = virt.subscribe();
        virt.power(id, action).await.unwrap();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("no event within 5 seconds")
                .unwrap();
            if let Event::Domain {
                id: got,
                change: DomainChange::Lifecycle { event, detail },
            } = event
                && got == id
                && event == VIR_DOMAIN_EVENT_STOPPED as i32
            {
                return detail;
            }
        }
    }

    #[tokio::test]
    async fn shutdown_and_force_off_stop_the_domain_in_different_ways() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = define(&virt, "power-detail").await;
        virt.power(id, Power::Start).await.unwrap();
        assert_eq!(
            stop_detail(&virt, id, Power::Shutdown).await,
            VIR_DOMAIN_EVENT_STOPPED_SHUTDOWN as i32
        );
        virt.power(id, Power::Start).await.unwrap();
        assert_eq!(
            stop_detail(&virt, id, Power::ForceOff).await,
            VIR_DOMAIN_EVENT_STOPPED_DESTROYED as i32
        );
    }

    #[tokio::test]
    async fn an_action_that_does_not_fit_the_state_fails() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = define(&virt, "power-invalid").await;
        for action in [Power::Shutdown, Power::ForceOff] {
            let err = virt.power(id, action).await.unwrap_err();
            assert!(err.is_invalid_operation(), "{action:?}: {err}");
        }
        virt.power(id, Power::Start).await.unwrap();
        let err = virt.power(id, Power::Start).await.unwrap_err();
        assert!(err.is_invalid_operation(), "{err}");
    }

    #[tokio::test]
    async fn an_unknown_domain_is_not_found() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let err = virt
            .power(Uuid::from_u128(0xdead), Power::Start)
            .await
            .unwrap_err();
        assert!(err.is_not_found(), "{err}");
    }
}
