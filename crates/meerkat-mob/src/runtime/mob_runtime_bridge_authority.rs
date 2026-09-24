use crate::AgentIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MobRuntimeBridgeEffect {
    DeliverLifecycleNotice {
        peer_id: AgentIdentity,
        intent: &'static str,
    },
}

pub(super) struct MobRuntimeBridgeAuthority;

impl MobRuntimeBridgeAuthority {
    pub(super) fn plan_lifecycle_notice(
        sender_ready: bool,
        wired_peers: &[AgentIdentity],
        intent: &'static str,
    ) -> Vec<MobRuntimeBridgeEffect> {
        if !sender_ready {
            return Vec::new();
        }
        wired_peers
            .iter()
            .cloned()
            .map(|peer_id| MobRuntimeBridgeEffect::DeliverLifecycleNotice { peer_id, intent })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sender_skips_notice_delivery() {
        let effects = MobRuntimeBridgeAuthority::plan_lifecycle_notice(
            false,
            &[AgentIdentity::from("a")],
            "mob.kickoff_failed",
        );
        assert!(effects.is_empty());
    }

    #[test]
    fn wired_peers_receive_bridge_notice_plan() {
        let effects = MobRuntimeBridgeAuthority::plan_lifecycle_notice(
            true,
            &[AgentIdentity::from("a"), AgentIdentity::from("b")],
            "mob.kickoff_failed",
        );
        assert_eq!(effects.len(), 2);
    }
}
