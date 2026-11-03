//! Capabilities preface for the console voice channel.
//!
//! The voice model is told nothing about the member it speaks for beyond the
//! platform's executor-split instructions, so it truthfully denied having
//! peers or tools. This module composes a short plain-text preface (identity,
//! peers, tool categories, skills) from what the host already holds at open,
//! and the console summary delivers it on the provider's instructions lane
//! together with the factual context summary. Names only, no schemas, and a
//! deterministic clamp so the preface can never crowd out the summary.

use std::collections::BTreeSet;

use async_trait::async_trait;
use meerkat::experimental_gpt_live::PublicGptLiveInstructionsPreface;
use meerkat_core::SessionId;
use meerkat_mob::{AgentIdentity, MobHandle, MobMemberStatus, ProfileBinding};

/// Hard ceiling for the composed preface in UTF-8 bytes.
pub(crate) const PREFACE_MAX_BYTES: usize = 2000;

/// One peer the member is connected to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PeerCapability {
    pub name: String,
    pub role: String,
}

/// Everything the preface is composed from. Plain data so the composer is
/// deterministic and unit-testable without a runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MemberCapabilities {
    pub identity: String,
    pub role: String,
    pub mob_id: String,
    pub peers: Vec<PeerCapability>,
    /// `(category, tool names)` in display order.
    pub tool_categories: Vec<(String, Vec<String>)>,
    pub skills: Vec<String>,
}

impl MemberCapabilities {
    /// Gather the member's capabilities from the mob handle. Peers are the
    /// member's live wiring targets when the roster knows them, else every
    /// other live member of the mob. Tools and skills come from the member's
    /// inline profile; a realm-referenced profile contributes only the
    /// categories the definition can see, which is none.
    fn gather(
        handle: &MobHandle,
        members: &[meerkat_mob::runtime::MobMemberListEntry],
        identity: &AgentIdentity,
    ) -> Option<Self> {
        let me = members
            .iter()
            .find(|member| &member.agent_identity == identity)?;
        let definition = handle.definition();
        let mut peers: Vec<PeerCapability> = if me.wired_to.is_empty() {
            members
                .iter()
                .filter(|member| member.agent_identity != *identity)
                .filter(|member| !matches!(member.status, MobMemberStatus::Retiring))
                .map(|member| PeerCapability {
                    name: member.agent_identity.to_string(),
                    role: member.role.to_string(),
                })
                .collect()
        } else {
            me.wired_to
                .iter()
                .map(|peer| PeerCapability {
                    name: peer.to_string(),
                    role: members
                        .iter()
                        .find(|member| &member.agent_identity == peer)
                        .map(|member| member.role.to_string())
                        .unwrap_or_default(),
                })
                .collect()
        };
        peers.sort();
        peers.dedup();
        let mut tool_categories = Vec::new();
        let mut skills = Vec::new();
        if let Some(ProfileBinding::Inline(profile)) = definition.profiles.get(&me.role) {
            let tools = &profile.tools;
            let mut categories: Vec<(&str, bool)> = vec![
                ("builtins", tools.builtins),
                ("shell", tools.shell),
                ("comms", tools.comms),
                ("memory", tools.memory),
                ("workgraph", tools.workgraph),
                ("mob", tools.mob),
                ("schedule", tools.schedule),
                ("image_generation", tools.image_generation),
            ];
            categories.retain(|(_, enabled)| *enabled);
            for (category, _) in categories {
                tool_categories.push((category.to_string(), Vec::new()));
            }
            if !tools.mcp.is_empty() {
                let mut modules: Vec<String> = tools.mcp.clone();
                modules.sort();
                modules.dedup();
                tool_categories.push(("mcp".to_string(), modules));
            }
            let unique: BTreeSet<String> = profile.skills.iter().cloned().collect();
            skills = unique.into_iter().collect();
        }
        Some(Self {
            identity: identity.to_string(),
            role: me.role.to_string(),
            mob_id: handle.mob_id().to_string(),
            peers,
            tool_categories,
            skills,
        })
    }

    /// Compose the preface. Identity and peers are kept first; tools and
    /// skills are truncated deterministically with an explicit "and N more"
    /// marker so the text never exceeds [`PREFACE_MAX_BYTES`].
    pub(crate) fn preface(&self) -> String {
        let mut text = format!(
            "You speak for {} ({}) in mob {}.",
            self.identity, self.role, self.mob_id
        );
        if self.peers.is_empty() {
            text.push_str(" You are not connected to other agents right now.");
        } else {
            text.push_str(" You are connected to: ");
            text.push_str(
                &self
                    .peers
                    .iter()
                    .map(|peer| {
                        if peer.role.is_empty() {
                            peer.name.clone()
                        } else {
                            format!("{} ({})", peer.name, peer.role)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push('.');
        }
        let closing = " When asked who you are connected to or what you can do, answer from this \
                       list. Anything that needs these tools runs through the executor and \
                       returns to you.";
        let budget = PREFACE_MAX_BYTES.saturating_sub(text.len() + closing.len());
        let mut tools_text = String::new();
        let mut omitted = 0usize;
        let mut entries: Vec<String> = Vec::new();
        for (category, names) in &self.tool_categories {
            if names.is_empty() {
                entries.push(category.clone());
            } else {
                entries.push(format!("{}: {}", category, names.join(", ")));
            }
        }
        if !entries.is_empty() {
            tools_text.push_str(" You have these tools available through the executor: ");
            let mut first = true;
            for (index, entry) in entries.iter().enumerate() {
                let separator = if first { "" } else { "; " };
                let remaining = entries.len() - index;
                let marker_reserve = if remaining > 1 { 24 } else { 0 };
                if tools_text.len() + separator.len() + entry.len() + 1 + marker_reserve
                    > budget.saturating_sub(self.skills_text_len())
                {
                    omitted = remaining;
                    break;
                }
                tools_text.push_str(separator);
                tools_text.push_str(entry);
                first = false;
            }
            if omitted > 0 {
                if !first {
                    tools_text.push_str("; ");
                }
                tools_text.push_str(&format!("and {omitted} more tool groups"));
            }
            tools_text.push('.');
        }
        text.push_str(&tools_text);
        if !self.skills.is_empty() {
            let skills = format!(" Skills: {}.", self.skills.join(", "));
            if text.len() + skills.len() + closing.len() <= PREFACE_MAX_BYTES {
                text.push_str(&skills);
            } else {
                text.push_str(&format!(" Skills: {} skills available.", self.skills.len()));
            }
        }
        text.push_str(closing);
        debug_assert!(text.len() <= PREFACE_MAX_BYTES + 64, "preface clamp failed");
        text
    }

    fn skills_text_len(&self) -> usize {
        if self.skills.is_empty() {
            0
        } else {
            " Skills: .".len()
                + self
                    .skills
                    .iter()
                    .map(|skill| skill.len() + 2)
                    .sum::<usize>()
        }
    }
}

/// Resolves the member behind a canonical session and composes its
/// capabilities preface. The shared live host holds one of these per mob and
/// consults it on every open, so each member's call carries its own roster.
pub(crate) struct MemberPreface {
    handle: MobHandle,
}

impl MemberPreface {
    pub(crate) fn new(handle: MobHandle) -> Self {
        Self { handle }
    }

    /// `None` when the session is not a current member's bridge session or
    /// the member has no inline profile to describe.
    pub(crate) async fn preface_for_session(&self, session_id: &SessionId) -> Option<String> {
        let members = self.handle.list_members().await;
        let owner = bridge_session_owner(&self.handle, &members, session_id).await?;
        MemberCapabilities::gather(&self.handle, &members, &owner)
            .map(|capabilities| capabilities.preface())
    }
}

/// The member whose bridge session is `session_id`. The roster observation
/// (a read lock, no machine-state clone) nominates the owner; one
/// authoritative machine-binding read confirms it. A lagging observation
/// yields `None`, never another member.
pub(crate) async fn bridge_session_owner(
    handle: &MobHandle,
    members: &[meerkat_mob::runtime::MobMemberListEntry],
    session_id: &SessionId,
) -> Option<AgentIdentity> {
    let mut nominated = None;
    for member in members {
        if handle
            .resolve_bridge_session_id_observation(&member.agent_identity)
            .await
            .as_ref()
            == Some(session_id)
        {
            if nominated.is_some() {
                return None;
            }
            nominated = Some(member.agent_identity.clone());
        }
    }
    let owner = nominated?;
    (handle.resolve_bridge_session_id(&owner).await.as_ref() == Some(session_id)).then_some(owner)
}

/// The open authority resolves this per canonical session, bounded by
/// meerkat's preface timeout, so the preface opens the session instructions
/// on its own carrier and never depends on the bootstrap summary.
#[async_trait]
impl PublicGptLiveInstructionsPreface for MemberPreface {
    async fn preface(&self, session_id: &SessionId) -> Option<String> {
        self.preface_for_session(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> MemberCapabilities {
        MemberCapabilities {
            identity: "agent-a".to_string(),
            role: "worker".to_string(),
            mob_id: "console-voice".to_string(),
            peers: vec![
                PeerCapability {
                    name: "agent-b".to_string(),
                    role: "reviewer".to_string(),
                },
                PeerCapability {
                    name: "ops-lead".to_string(),
                    role: "orchestrator".to_string(),
                },
            ],
            tool_categories: vec![
                ("builtins".to_string(), Vec::new()),
                ("shell".to_string(), Vec::new()),
                (
                    "mcp".to_string(),
                    vec!["github".to_string(), "ledger".to_string()],
                ),
            ],
            skills: vec!["incident-triage".to_string(), "release-notes".to_string()],
        }
    }

    #[test]
    fn preface_is_deterministic_and_names_everything() {
        let text = fixture().preface();
        assert_eq!(
            text,
            "You speak for agent-a (worker) in mob console-voice. You are connected to: \
             agent-b (reviewer), ops-lead (orchestrator). You have these tools available \
             through the executor: builtins; shell; mcp: github, ledger. Skills: \
             incident-triage, release-notes. When asked who you are connected to or what you \
             can do, answer from this list. Anything that needs these tools runs through the \
             executor and returns to you."
        );
        assert_eq!(fixture().preface(), text);
    }

    #[test]
    fn preface_without_peers_says_so() {
        let mut caps = fixture();
        caps.peers.clear();
        assert!(
            caps.preface()
                .contains("You are not connected to other agents right now.")
        );
    }

    #[test]
    fn preface_clamps_tools_and_keeps_identity_and_peers_first() {
        let mut caps = fixture();
        caps.tool_categories = (0..400)
            .map(|index| {
                (
                    format!("category-{index:03}"),
                    vec![format!("tool-{index:03}")],
                )
            })
            .collect();
        caps.skills = (0..200).map(|index| format!("skill-{index:03}")).collect();
        let text = caps.preface();
        assert!(text.len() <= PREFACE_MAX_BYTES + 64, "{}", text.len());
        assert!(text.starts_with("You speak for agent-a (worker) in mob console-voice."));
        assert!(text.contains("agent-b (reviewer), ops-lead (orchestrator)."));
        assert!(text.contains("and "), "omission marker missing: {text}");
        assert!(text.contains("more tool groups"), "{text}");
        assert!(text.contains("Skills: 200 skills available."), "{text}");
        assert!(text.ends_with("returns to you."));
    }
}
