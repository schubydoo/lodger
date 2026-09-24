//! Power actions and autostart (PRD F4, TAD 9.5). Each action is one
//! libvirt call. The new state reaches the UI through the lifecycle event,
//! not through the return value.

use lodger_core::model::VmState;
use uuid::Uuid;
use virt::domain::Domain;
use virt::error::ErrorNumber;
use virt::sys::{VIR_DOMAIN_REBOOT_ACPI_POWER_BTN, VIR_DOMAIN_SHUTDOWN_ACPI_POWER_BTN};

use crate::conn::{Error, Virt};
use crate::events::{DomainChange, Event};

/// A power action on a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Start,
    /// Asks the guest to shut down with the ACPI power button. The domain
    /// stops when the guest finishes.
    Shutdown,
    /// Stops the domain at once, like pulling the power cable.
    ForceOff,
    /// Asks the guest to restart with the ACPI power button.
    Reboot,
    /// Stops the guest's CPUs. Its memory stays.
    Pause,
    /// Runs the CPUs of a paused guest again.
    Resume,
}

impl Power {
    /// The action from its name in the API path.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "start" => Some(Self::Start),
            "shutdown" => Some(Self::Shutdown),
            "force-off" => Some(Self::ForceOff),
            "reboot" => Some(Self::Reboot),
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            _ => None,
        }
    }

    /// The name in the API path and in the audit log.
    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Shutdown => "shutdown",
            Self::ForceOff => "force-off",
            Self::Reboot => "reboot",
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }

    /// Why the action does not fit a domain in `state`, if it does not.
    fn refusal(self, state: VmState, active: bool) -> Option<&'static str> {
        match self {
            Self::Start if active => Some("the VM is running already"),
            Self::Shutdown | Self::ForceOff if !active => Some("the VM is not running"),
            Self::Reboot | Self::Pause if state == VmState::Paused => Some("the VM is paused"),
            Self::Reboot | Self::Pause if state != VmState::Running => {
                Some("the VM is not running")
            }
            Self::Resume if state != VmState::Paused => Some("the VM is not paused"),
            _ => None,
        }
    }
}

/// Runs a call with the ACPI power button flag. A driver without ACPI, such
/// as libvirt's test driver, rejects the flag: then the driver picks its own
/// method.
fn with_acpi(
    call: impl Fn(u32) -> Result<(), virt::error::Error>,
    acpi: u32,
) -> Result<(), virt::error::Error> {
    match call(acpi) {
        Err(e) if e.code().known() == Some(ErrorNumber::InvalidArg) => call(0),
        other => other,
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
            let state = VmState::from_code(domain.info()?.state.to_raw());
            if let Some(reason) = action.refusal(state, domain.is_active()?) {
                return Ok(Err(Error::WrongState(reason)));
            }
            let done = match action {
                Power::Start => domain.create().map(drop),
                Power::Shutdown => with_acpi(
                    |flags| domain.shutdown_flags(flags),
                    VIR_DOMAIN_SHUTDOWN_ACPI_POWER_BTN,
                ),
                Power::ForceOff => domain.destroy(),
                Power::Reboot => with_acpi(
                    |flags| domain.reboot(flags),
                    VIR_DOMAIN_REBOOT_ACPI_POWER_BTN,
                ),
                Power::Pause => domain.suspend(),
                Power::Resume => domain.resume(),
            };
            Ok(done.map_err(Error::from))
        })
        .await?
    }

    /// Switches autostart of domain `id` on or off. libvirt sends no event
    /// for this, so Lodger sends its own `Autostart` change to the hub, and
    /// the inventory and the UI refresh as for any other change.
    pub async fn set_autostart(&self, id: Uuid, on: bool) -> Result<(), Error> {
        self.job(move |c| {
            let domain: Domain = c.lookup_domain_by_uuid(id)?;
            domain.set_autostart(on)
        })
        .await?;
        let _ = self.hub.send(Event::Domain {
            id,
            change: DomainChange::Autostart,
        });
        Ok(())
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

    use lodger_core::model::VmState;

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
        for action in [
            Power::Start,
            Power::Shutdown,
            Power::ForceOff,
            Power::Reboot,
            Power::Pause,
            Power::Resume,
        ] {
            assert_eq!(Power::parse(action.name()), Some(action));
        }
        assert_eq!(Power::parse("suspend"), None);
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

    async fn state(virt: &Virt, id: Uuid) -> VmState {
        virt.read(move |c| {
            let info = c.lookup_domain_by_uuid(id)?.info()?;
            Ok(VmState::from_code(info.state.to_raw()))
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn pause_resume_and_reboot_work_on_a_running_domain() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = define(&virt, "power-pause").await;
        virt.power(id, Power::Start).await.unwrap();
        virt.power(id, Power::Pause).await.unwrap();
        assert_eq!(state(&virt, id).await, VmState::Paused);
        virt.power(id, Power::Resume).await.unwrap();
        assert_eq!(state(&virt, id).await, VmState::Running);
        // The test driver sends no event for a reboot, but it changes the
        // state reason from "unpaused" to "booted".
        let reason = |virt: Virt| async move {
            virt.read(move |c| Ok(format!("{:?}", c.lookup_domain_by_uuid(id)?.state()?.1)))
                .await
                .unwrap()
        };
        assert!(reason(virt.clone()).await.contains("Unpaused"));
        virt.power(id, Power::Reboot).await.unwrap();
        assert!(reason(virt.clone()).await.contains("Booted"));
        assert_eq!(state(&virt, id).await, VmState::Running);
    }

    #[test]
    fn each_action_fits_only_its_states() {
        use VmState::{Paused, Running, Shutoff};
        let cases = [
            (Power::Start, Shutoff, false, None),
            (
                Power::Start,
                Running,
                true,
                Some("the VM is running already"),
            ),
            (Power::Shutdown, Paused, true, None),
            (
                Power::ForceOff,
                Shutoff,
                false,
                Some("the VM is not running"),
            ),
            (Power::Reboot, Running, true, None),
            (Power::Reboot, Paused, true, Some("the VM is paused")),
            (Power::Reboot, Shutoff, false, Some("the VM is not running")),
            (Power::Pause, Running, true, None),
            (Power::Pause, Paused, true, Some("the VM is paused")),
            (Power::Pause, Shutoff, false, Some("the VM is not running")),
            (Power::Resume, Paused, true, None),
            (Power::Resume, Running, true, Some("the VM is not paused")),
            (Power::Resume, Shutoff, false, Some("the VM is not paused")),
        ];
        for (action, state, active, want) in cases {
            assert_eq!(action.refusal(state, active), want, "{action:?} {state:?}");
        }
    }

    #[tokio::test]
    async fn the_wrong_state_fails_before_libvirt() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = define(&virt, "power-wrong").await;
        for action in [Power::Reboot, Power::Pause, Power::Resume] {
            let err = virt.power(id, action).await.unwrap_err();
            assert!(err.is_invalid_operation(), "{action:?}: {err}");
        }
    }

    #[tokio::test]
    async fn autostart_changes_and_sends_its_own_event() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = define(&virt, "power-autostart").await;
        let autostart = |virt: Virt| async move {
            virt.read(move |c| c.lookup_domain_by_uuid(id)?.autostart())
                .await
                .unwrap()
        };
        let mut events = virt.subscribe();
        virt.set_autostart(id, true).await.unwrap();
        assert!(autostart(virt.clone()).await);
        // Other tests share the test driver, so skip their events.
        let want = Event::Domain {
            id,
            change: DomainChange::Autostart,
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while events.recv().await.unwrap() != want {}
        })
        .await
        .expect("no autostart event within 5 seconds");
        virt.set_autostart(id, false).await.unwrap();
        assert!(!autostart(virt.clone()).await);
        let err = virt
            .set_autostart(Uuid::from_u128(0xdead), true)
            .await
            .unwrap_err();
        assert!(err.is_not_found(), "{err}");
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
