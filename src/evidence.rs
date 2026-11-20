//! Bounded rule-local proof and independent alert policy. No heap allocation.
use crate::rules::{ExprNode, RuleDb, SigIdx, SigMatchSet};
use heapless::Vec;
use serde::Serialize;

/// Evidence records intentionally omit packet contents; IDs resolve in the pack.
#[derive(Clone, Debug, Serialize)]
pub struct RuleEvidence {
    pub id: u16,
    pub sig: Vec<SigIdx, 4>,
    pub more: bool,
    #[serde(skip)]
    pub alert_eligible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertPolicy {
    RulesAndWatchlist,
    WatchlistOnly,
    AllMatches,
    Silent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertDecision {
    None,
    Suppressed,
    Watchlist,
    Rule,
    Match,
}
impl AlertDecision {
    pub fn fires(self) -> bool {
        matches!(self, Self::Watchlist | Self::Rule | Self::Match)
    }
}
impl AlertPolicy {
    pub fn decide(self, matched: bool, watchlist: bool, rules: &[RuleEvidence]) -> AlertDecision {
        if !matched {
            return AlertDecision::None;
        }
        if self == Self::Silent {
            return AlertDecision::Suppressed;
        }
        if watchlist {
            return AlertDecision::Watchlist;
        }
        if self == Self::AllMatches {
            return AlertDecision::Match;
        }
        if self == Self::RulesAndWatchlist
            && rules
                .iter()
                .any(|r| r.alert_eligible && !r.more && !r.sig.is_empty())
        {
            return AlertDecision::Rule;
        }
        AlertDecision::Suppressed
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Truth {
    Yes,
    No,
    Unknown,
}
struct Proof {
    truth: Truth,
    sig: Vec<SigIdx, 4>,
    more: bool,
}
impl Proof {
    fn new(truth: Truth) -> Self {
        Self {
            truth,
            sig: Vec::new(),
            more: false,
        }
    }
    fn merge(&mut self, other: &Self) {
        self.more |= other.more;
        for &id in &other.sig {
            if !self.sig.contains(&id) && self.sig.push(id).is_err() {
                self.more = true;
            }
        }
    }
}

/// Observed positives only. Missing predicates are unknown, so NOT cannot turn
/// missing/truncated/backend-unavailable fields into positive evidence. Definitive
/// absence needs a future completeness-aware predicate contract.
fn proof(nodes: &[ExprNode], matched: &SigMatchSet) -> Option<Proof> {
    let mut stack: Vec<Proof, 16> = Vec::new();
    for node in nodes {
        match *node {
            ExprNode::Sig(id) => {
                let mut value = Proof::new(if matched.get(id) {
                    Truth::Yes
                } else {
                    Truth::Unknown
                });
                if value.truth == Truth::Yes {
                    value.sig.push(id).ok()?;
                }
                stack.push(value).ok()?;
            }
            ExprNode::Not => {
                let value = stack.last_mut()?;
                value.truth = match value.truth {
                    Truth::Yes => Truth::No,
                    Truth::No => Truth::Yes,
                    Truth::Unknown => Truth::Unknown,
                };
                // Negative clauses are constraints, not independent positive clues.
                value.sig.clear();
                value.more = false;
            }
            ExprNode::AnyOf { count } | ExprNode::AllOf { count } => {
                let count = usize::from(count);
                if count == 0 || count > stack.len() {
                    return None;
                }
                let start = stack.len() - count;
                let children = &stack[start..];
                let any = matches!(node, ExprNode::AnyOf { .. });
                let truth = if any {
                    if children.iter().any(|p| p.truth == Truth::Yes) {
                        Truth::Yes
                    } else if children.iter().all(|p| p.truth == Truth::No) {
                        Truth::No
                    } else {
                        Truth::Unknown
                    }
                } else if children.iter().any(|p| p.truth == Truth::No) {
                    Truth::No
                } else if children.iter().all(|p| p.truth == Truth::Yes) {
                    Truth::Yes
                } else {
                    Truth::Unknown
                };
                let mut value = Proof::new(truth);
                if truth == Truth::Yes {
                    if any {
                        // A single satisfied branch is the proof; unrelated OR
                        // alternatives cannot upgrade it into corroboration.
                        value.merge(children.iter().find(|p| p.truth == Truth::Yes)?);
                    } else {
                        for child in children {
                            value.merge(child);
                        }
                    }
                }
                stack.truncate(start);
                stack.push(value).ok()?;
            }
        }
    }
    if stack.len() != 1 {
        return None;
    }
    let result = stack.pop()?;
    (result.truth == Truth::Yes && !result.sig.is_empty()).then_some(result)
}

pub fn evaluate(db: &RuleDb, matched: &SigMatchSet) -> (Vec<RuleEvidence, 4>, bool) {
    let mut results = Vec::new();
    let mut more = false;
    for (index, rule) in db.rules.iter().enumerate() {
        if !rule.active {
            continue;
        }
        let start = usize::from(rule.expr_start);
        let end = start + usize::from(rule.expr_len);
        let Some(nodes) = db.nodes.get(start..end) else {
            continue;
        };
        let Some(proof) = proof(nodes, matched) else {
            continue;
        };
        let Ok(id) = u16::try_from(index) else {
            more = true;
            continue;
        };
        if results
            .push(RuleEvidence {
                id,
                sig: proof.sig,
                more: proof.more,
                alert_eligible: rule.alert,
            })
            .is_err()
        {
            more = true;
        }
    }
    (results, more)
}

/// Compact companion record; no observation payload or queue layout expansion.
#[derive(Debug, Serialize)]
pub struct Detection<'a> {
    pub engine: &'static str,
    pub policy: AlertPolicy,
    pub pack: &'static str,
    pub ruleset: &'static str,
    pub sig: Vec<SigIdx, 8>,
    pub rules: &'a [RuleEvidence],
    pub more: bool,
    pub basis: &'static str,
    pub alert: AlertDecision,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::rules::{MatchCategory, Rule};
    fn matches(ids: &[u16]) -> SigMatchSet {
        let mut set = SigMatchSet::new();
        for &id in ids {
            set.set(id);
        }
        set
    }
    const fn rule(start: u16, len: u16, active: bool, alert: bool) -> Rule {
        Rule {
            name: "candidate",
            expr_start: start,
            expr_len: len,
            category: MatchCategory::Network,
            active,
            alert,
        }
    }
    #[test]
    fn branch_local_proof_deduplicates_without_borrowing_other_matches() {
        static NODES: &[ExprNode] = &[
            ExprNode::Sig(1),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::AllOf { count: 3 },
            ExprNode::Sig(3),
            ExprNode::AnyOf { count: 2 },
        ];
        let p = proof(NODES, &matches(&[1, 2, 3, 99])).unwrap();
        assert_eq!(&p.sig[..], &[1, 2]);
        assert!(!p.more);
        assert_eq!(&proof(NODES, &matches(&[3, 99])).unwrap().sig[..], &[3]);
    }
    #[test]
    fn missing_and_negative_fields_do_not_become_evidence() {
        assert!(proof(&[ExprNode::Sig(1), ExprNode::Not], &matches(&[])).is_none());
        assert!(proof(&[ExprNode::Sig(1), ExprNode::Not], &matches(&[1])).is_none());
        assert!(proof(
            &[ExprNode::Sig(1), ExprNode::Not, ExprNode::Not],
            &matches(&[1])
        )
        .is_none());
        assert!(proof(
            &[
                ExprNode::Sig(1),
                ExprNode::Sig(2),
                ExprNode::AllOf { count: 2 }
            ],
            &matches(&[1])
        )
        .is_none());
        assert!(proof(
            &[
                ExprNode::Sig(1),
                ExprNode::Not,
                ExprNode::Sig(2),
                ExprNode::AllOf { count: 2 }
            ],
            &matches(&[1, 2])
        )
        .is_none());
    }
    #[test]
    fn malformed_and_overdeep_expressions_fail_closed() {
        for nodes in [
            &[][..],
            &[ExprNode::Not][..],
            &[ExprNode::AnyOf { count: 0 }][..],
            &[ExprNode::AllOf { count: 2 }][..],
            &[ExprNode::Sig(1), ExprNode::Sig(2)][..],
            &[ExprNode::Sig(1); 17][..],
        ] {
            assert!(proof(nodes, &matches(&[1, 2])).is_none());
        }
        assert!(proof(
            &[
                ExprNode::Sig(1),
                ExprNode::Not,
                ExprNode::Sig(2),
                ExprNode::Not,
                ExprNode::AnyOf { count: 2 }
            ],
            &matches(&[1, 2])
        )
        .is_none());
    }
    #[test]
    fn disabled_invalid_and_custom_rules_keep_their_own_policy() {
        static NODES: &[ExprNode] = &[ExprNode::Sig(1), ExprNode::Sig(2)];
        static RULES: &[Rule] = &[
            rule(0, 1, false, true),
            rule(99, 1, true, true),
            rule(1, 1, true, false),
        ];
        let db = RuleDb {
            id: "custom-v1",
            nodes: NODES,
            rules: RULES,
        };
        let (e, more) = evaluate(&db, &matches(&[1, 2]));
        assert!(!more);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].id, 2);
        assert_eq!(&e[0].sig[..], &[2]);
        assert_eq!(
            AlertPolicy::RulesAndWatchlist.decide(true, false, &e),
            AlertDecision::Suppressed
        );
    }
    #[test]
    fn evidence_overflow_is_explicit_and_cannot_enable_alerts() {
        static NODES: &[ExprNode] = &[
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::Sig(3),
            ExprNode::Sig(4),
            ExprNode::AllOf { count: 5 },
        ];
        static RULES: &[Rule] = &[rule(0, 6, true, true); 5];
        let (e, more) = evaluate(
            &RuleDb {
                id: "large",
                nodes: NODES,
                rules: RULES,
            },
            &matches(&[0, 1, 2, 3, 4]),
        );
        assert!(more);
        assert_eq!(e.len(), 4);
        assert!(e.iter().all(|p| p.more && p.sig.len() == 4));
        assert!(!AlertPolicy::RulesAndWatchlist
            .decide(true, false, &e)
            .fires());
    }
    #[test]
    fn watchlist_is_user_intent_not_rule_corroboration() {
        let e = RuleEvidence {
            id: 0,
            sig: Vec::from_slice(&[1]).unwrap(),
            more: false,
            alert_eligible: true,
        };
        assert_eq!(
            AlertPolicy::RulesAndWatchlist.decide(true, false, &[e]),
            AlertDecision::Rule
        );
        assert_eq!(
            AlertPolicy::RulesAndWatchlist.decide(true, true, &[]),
            AlertDecision::Watchlist
        );
        assert_eq!(
            AlertPolicy::WatchlistOnly.decide(true, false, &[]),
            AlertDecision::Suppressed
        );
        assert_eq!(
            AlertPolicy::AllMatches.decide(true, false, &[]),
            AlertDecision::Match
        );
        assert_eq!(
            AlertPolicy::Silent.decide(true, true, &[]),
            AlertDecision::Suppressed
        );
        assert_eq!(
            AlertPolicy::AllMatches.decide(false, false, &[]),
            AlertDecision::None
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod wire_tests {
    use super::*;
    #[test]
    fn maximum_builtin_evidence_fits_firmware_line() {
        let mut rules = Vec::<RuleEvidence, 4>::new();
        for id in 0..4 {
            rules
                .push(RuleEvidence {
                    id,
                    sig: Vec::from_slice(&[252, 253, 254, 255]).unwrap(),
                    more: true,
                    alert_eligible: false,
                })
                .unwrap();
        }
        let message = crate::protocol::DeviceMessage::Detection {
            id: "FF:FF:FF:FF:FF:FF",
            scope: "wifi_address",
            ts: u32::MAX,
            detection: Detection {
                engine: "evidence-1",
                policy: AlertPolicy::RulesAndWatchlist,
                pack: crate::defaults::SIGNATURE_PACK_ID,
                ruleset: crate::defaults::DEFAULT_RULE_DB.id,
                sig: Vec::from_slice(&[248, 249, 250, 251, 252, 253, 254, 255]).unwrap(),
                rules: &rules,
                more: true,
                basis: "observed_fields",
                alert: AlertDecision::Suppressed,
            },
        };
        let mut buffer = [0; crate::protocol::MAX_MSG_LEN];
        let len = crate::comm::serialize_message(&message, &mut buffer)
            .expect("bounded evidence must fit with newline");
        assert!(len <= crate::protocol::MAX_MSG_LEN);
    }
}
