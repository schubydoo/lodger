//! Known libvirt errors, each with its cause and its fix (PRD R10, TAD 4.3).
//!
//! libvirt reports some host problems with a message that names only the
//! symptom. For example, `AppArmor` on Debian 13 blocks `/dev/vhost-net`, and
//! QEMU says only that `getfd` got no file descriptor. Each entry here
//! matches the fixed part of one message. An error that matches no entry
//! keeps its libvirt text, and the UI shows it unchanged.

/// Why a known error happened, and how to fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Explanation {
    /// The cause, as one sentence.
    pub cause: &'static str,
    /// What to do, as one sentence.
    pub fix: &'static str,
    /// Commands that do the fix on the host, in order. Empty if the fix is
    /// a step in Lodger.
    pub commands: &'static [&'static str],
}

/// One known error: every text in `parts` appears in the libvirt message.
struct Known {
    parts: &'static [&'static str],
    explanation: Explanation,
}

const KNOWN: [Known; 2] = [
    // A virtio NIC hot-plug on Debian 13, from Phase 0. The exact message,
    // from libvirt 10.0.0 with the rule removed: "internal error: unable to
    // execute QEMU command 'getfd': No file descriptor supplied via
    // SCM_RIGHTS".
    Known {
        parts: &["QEMU command 'getfd'", "No file descriptor supplied"],
        explanation: Explanation {
            cause: "AppArmor on this host blocks /dev/vhost-net, so QEMU cannot get the network device for the new NIC.",
            fix: "Add the rule `/dev/vhost-net rw,` to /etc/apparmor.d/local/abstractions/libvirt-qemu, then stop and start the VM, because a VM gets the rule at its next start.",
            commands: &[
                "sudo mkdir -p /etc/apparmor.d/local/abstractions",
                "printf '\\n/dev/vhost-net rw,\\n' | sudo tee -a /etc/apparmor.d/local/abstractions/libvirt-qemu",
            ],
        },
    },
    // A delete of the current external snapshot after a revert to it, from
    // Phase 0. The exact message, from libvirt 10.0.0: "unsupported
    // configuration: deletion of active external snapshot that is not a leaf
    // snapshot is not supported".
    Known {
        parts: &["deletion of active external snapshot that is not a leaf snapshot"],
        explanation: Explanation {
            cause: "libvirt cannot delete the current external snapshot while it has a child snapshot, for example right after a revert to it.",
            fix: "Delete its child snapshots first, or revert to another snapshot, then delete this one.",
            commands: &[],
        },
    },
];

/// The explanation of a libvirt error message, if the message is known.
pub fn explain(message: &str) -> Option<&'static Explanation> {
    KNOWN
        .iter()
        .find(|known| known.parts.iter().all(|part| message.contains(part)))
        .map(|known| &known.explanation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_getfd_error_names_the_apparmor_rule_and_its_2_commands() {
        let message = "internal error: unable to execute QEMU command 'getfd': \
                       No file descriptor supplied via SCM_RIGHTS";
        let explanation = explain(message).expect("the getfd error is known");
        assert!(explanation.cause.contains("AppArmor"));
        assert!(explanation.fix.contains("`/dev/vhost-net rw,`"));
        assert_eq!(
            explanation.commands,
            [
                "sudo mkdir -p /etc/apparmor.d/local/abstractions",
                "printf '\\n/dev/vhost-net rw,\\n' | sudo tee -a /etc/apparmor.d/local/abstractions/libvirt-qemu",
            ]
        );
    }

    #[test]
    fn a_getfd_error_for_another_reason_is_not_matched() {
        let message = "internal error: unable to execute QEMU command 'getfd': Device busy";
        assert_eq!(explain(message), None);
    }

    #[test]
    fn the_snapshot_delete_rule_says_to_delete_the_children_first() {
        let message = "unsupported configuration: deletion of active external snapshot \
                       that is not a leaf snapshot is not supported";
        let explanation = explain(message).expect("the snapshot rule is known");
        assert!(explanation.cause.contains("child snapshot"));
        assert!(
            explanation
                .fix
                .starts_with("Delete its child snapshots first")
        );
        assert!(explanation.commands.is_empty());
    }

    #[test]
    fn an_unknown_error_has_no_explanation() {
        for message in [
            "",
            "Domain not found",
            "operation failed: domain is already running",
        ] {
            assert_eq!(explain(message), None, "{message}");
        }
    }

    #[test]
    fn every_entry_matches_its_own_parts() {
        for known in &KNOWN {
            assert_eq!(
                explain(&known.parts.join(" ")),
                Some(&known.explanation),
                "{:?}",
                known.parts
            );
            assert!(known.explanation.cause.ends_with('.'));
            assert!(known.explanation.fix.ends_with('.'));
        }
    }
}
