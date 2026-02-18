/// Boolean rule composition engine for named device detection.
///
/// Rules compose signatures with `anyOf`/`allOf`/`not` boolean logic,
/// enabling the firmware to report "matched a Flock Safety Camera" rather
/// than just "matched a Flock Safety MAC OUI."
///
/// The expression tree uses a **flat post-order encoding** — children precede
/// their parent in the array. Evaluated with a fixed-size bool stack.
/// Zero allocation, `no_std` compatible.
use heapless::Vec;

/// Index into the signature arrays in `defaults.rs`.
pub type SigIdx = u16;

/// Maximum number of distinct signatures the bitset can track.
pub const MAX_SIGNATURES: usize = 256;

/// Maximum depth of the evaluation stack (deepest realistic nesting).
const MAX_EVAL_STACK: usize = 16;

/// A node in a post-order expression tree.
///
/// Expressions are stored as flat arrays where children always precede
/// their parent. For example, `anyOf(sig0, allOf(sig1, sig2))` encodes as:
/// `[Sig(0), Sig(1), Sig(2), AllOf{count:2}, AnyOf{count:2}]`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprNode {
    /// Leaf: true if the signature at this index was matched.
    Sig(SigIdx),
    /// OR over the preceding `count` results on the stack.
    AnyOf { count: u8 },
    /// AND over the preceding `count` results on the stack.
    AllOf { count: u8 },
    /// Invert the preceding result on the stack.
    Not,
}

/// A 256-bit bitset tracking which signatures matched in a scan event.
///
/// Each bit position corresponds to a `SigIdx`. Constant-time set/get.
/// Stack-only — 32 bytes.
#[derive(Debug, Clone)]
pub struct SigMatchSet {
    bits: [u64; 4],
}

impl SigMatchSet {
    /// Create an empty match set (no signatures matched).
    pub const fn new() -> Self {
        Self { bits: [0; 4] }
    }

    /// Mark a signature index as matched.
    ///
    /// Indices >= MAX_SIGNATURES are silently ignored.
    #[inline]
    pub fn set(&mut self, idx: SigIdx) {
        let i = idx as usize;
        if i < MAX_SIGNATURES {
            self.bits[i / 64] |= 1u64 << (i % 64);
        }
    }

    /// Check if a signature index is marked as matched.
    #[inline]
    pub fn get(&self, idx: SigIdx) -> bool {
        let i = idx as usize;
        if i < MAX_SIGNATURES {
            (self.bits[i / 64] >> (i % 64)) & 1 == 1
        } else {
            false
        }
    }

    /// Returns true if no signatures are matched.
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }
}

/// A named rule that references a slice of the shared expression node pool.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Human-readable rule name (e.g., "Flock Safety Camera").
    pub name: &'static str,
    /// Starting offset into the shared `RuleDb::nodes` array.
    pub expr_start: u16,
    /// Number of expression nodes for this rule.
    pub expr_len: u16,
}

/// A compiled rule database: a shared pool of expression nodes and
/// an array of rules that reference slices of the pool.
#[derive(Debug, Clone, Copy)]
pub struct RuleDb {
    /// Shared pool of expression nodes (all rules' nodes concatenated).
    pub nodes: &'static [ExprNode],
    /// Array of rule definitions referencing slices of `nodes`.
    pub rules: &'static [Rule],
}

/// Evaluate a single expression (a slice of post-order `ExprNode`s)
/// against the matched signature set.
///
/// Returns `true` if the expression evaluates to true.
/// Returns `false` for empty expressions or on stack overflow.
pub fn evaluate_rule(nodes: &[ExprNode], matched: &SigMatchSet) -> bool {
    if nodes.is_empty() {
        return false;
    }

    let mut stack: Vec<bool, MAX_EVAL_STACK> = Vec::new();

    for &node in nodes {
        match node {
            ExprNode::Sig(idx) => {
                if stack.push(matched.get(idx)).is_err() {
                    return false; // stack overflow
                }
            }
            ExprNode::AnyOf { count } => {
                let count = count as usize;
                if stack.len() < count {
                    return false; // underflow
                }
                let start = stack.len() - count;
                let result = stack[start..].iter().any(|&v| v);
                stack.truncate(start);
                if stack.push(result).is_err() {
                    return false;
                }
            }
            ExprNode::AllOf { count } => {
                let count = count as usize;
                if stack.len() < count {
                    return false; // underflow
                }
                let start = stack.len() - count;
                let result = stack[start..].iter().all(|&v| v);
                stack.truncate(start);
                if stack.push(result).is_err() {
                    return false;
                }
            }
            ExprNode::Not => {
                if let Some(val) = stack.pop() {
                    if stack.push(!val).is_err() {
                        return false;
                    }
                } else {
                    return false; // underflow
                }
            }
        }
    }

    // The final result should be exactly one value on the stack
    stack.len() == 1 && stack[0]
}

/// Evaluate all rules in a database against the matched signature set.
///
/// Returns indices (into `db.rules`) of rules that matched, up to 4.
pub fn evaluate_rules(db: &RuleDb, matched: &SigMatchSet) -> Vec<u16, 4> {
    let mut result = Vec::new();

    for (i, rule) in db.rules.iter().enumerate() {
        let start = rule.expr_start as usize;
        let end = start + rule.expr_len as usize;

        if end <= db.nodes.len() {
            let nodes = &db.nodes[start..end];
            if evaluate_rule(nodes, matched) {
                let _ = result.push(i as u16);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SigMatchSet tests ───────────────────────────────────────────

    #[test]
    fn sigmatchset_empty_on_creation() {
        let set = SigMatchSet::new();
        assert!(set.is_empty());
        for i in 0..256u16 {
            assert!(!set.get(i), "bit {i} should be unset");
        }
    }

    #[test]
    fn sigmatchset_set_and_get_bit_zero() {
        let mut set = SigMatchSet::new();
        set.set(0);
        assert!(set.get(0));
        assert!(!set.is_empty());
    }

    #[test]
    fn sigmatchset_set_and_get_bit_63() {
        let mut set = SigMatchSet::new();
        set.set(63);
        assert!(set.get(63));
        assert!(!set.get(62));
        assert!(!set.get(64));
    }

    #[test]
    fn sigmatchset_set_and_get_bit_64() {
        let mut set = SigMatchSet::new();
        set.set(64);
        assert!(set.get(64));
        assert!(!set.get(63));
        assert!(!set.get(65));
    }

    #[test]
    fn sigmatchset_set_and_get_bit_127() {
        let mut set = SigMatchSet::new();
        set.set(127);
        assert!(set.get(127));
        assert!(!set.get(126));
        assert!(!set.get(128));
    }

    #[test]
    fn sigmatchset_set_and_get_bit_128() {
        let mut set = SigMatchSet::new();
        set.set(128);
        assert!(set.get(128));
    }

    #[test]
    fn sigmatchset_set_and_get_bit_255() {
        let mut set = SigMatchSet::new();
        set.set(255);
        assert!(set.get(255));
        assert!(!set.get(254));
    }

    #[test]
    fn sigmatchset_out_of_range_ignored() {
        let mut set = SigMatchSet::new();
        set.set(256); // should be silently ignored
        set.set(1000);
        assert!(set.is_empty());
        assert!(!set.get(256));
        assert!(!set.get(1000));
    }

    #[test]
    fn sigmatchset_multiple_bits_set() {
        let mut set = SigMatchSet::new();
        set.set(0);
        set.set(42);
        set.set(100);
        set.set(200);
        set.set(255);
        assert!(set.get(0));
        assert!(set.get(42));
        assert!(set.get(100));
        assert!(set.get(200));
        assert!(set.get(255));
        // Spot-check unset values
        assert!(!set.get(1));
        assert!(!set.get(41));
        assert!(!set.get(99));
        assert!(!set.get(201));
        assert!(!set.get(254));
    }

    #[test]
    fn sigmatchset_set_idempotent() {
        let mut set = SigMatchSet::new();
        set.set(10);
        set.set(10);
        set.set(10);
        assert!(set.get(10));
    }

    #[test]
    fn sigmatchset_all_word_boundaries() {
        let mut set = SigMatchSet::new();
        // Set bits at each u64 boundary
        for &idx in &[0u16, 63, 64, 127, 128, 191, 192, 255] {
            set.set(idx);
        }
        for &idx in &[0u16, 63, 64, 127, 128, 191, 192, 255] {
            assert!(set.get(idx), "bit {idx} should be set");
        }
        // Check that adjacent bits are NOT set
        for &idx in &[1u16, 62, 65, 126, 129, 190, 193, 254] {
            assert!(!set.get(idx), "bit {idx} should NOT be set");
        }
    }

    // ── evaluate_rule: single Sig leaf ──────────────────────────────

    #[test]
    fn eval_single_sig_match() {
        let mut matched = SigMatchSet::new();
        matched.set(5);
        let nodes = [ExprNode::Sig(5)];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_single_sig_no_match() {
        let matched = SigMatchSet::new();
        let nodes = [ExprNode::Sig(5)];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_single_sig_wrong_index() {
        let mut matched = SigMatchSet::new();
        matched.set(3);
        let nodes = [ExprNode::Sig(5)];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    // ── evaluate_rule: AnyOf ────────────────────────────────────────

    #[test]
    fn eval_anyof_first_matches() {
        let mut matched = SigMatchSet::new();
        matched.set(0);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::AnyOf { count: 3 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_anyof_last_matches() {
        let mut matched = SigMatchSet::new();
        matched.set(2);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::AnyOf { count: 3 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_anyof_all_match() {
        let mut matched = SigMatchSet::new();
        matched.set(0);
        matched.set(1);
        matched.set(2);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::AnyOf { count: 3 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_anyof_none_match() {
        let matched = SigMatchSet::new();
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::AnyOf { count: 3 },
        ];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_anyof_single_child() {
        let mut matched = SigMatchSet::new();
        matched.set(7);
        let nodes = [ExprNode::Sig(7), ExprNode::AnyOf { count: 1 }];
        assert!(evaluate_rule(&nodes, &matched));
    }

    // ── evaluate_rule: AllOf ────────────────────────────────────────

    #[test]
    fn eval_allof_all_match() {
        let mut matched = SigMatchSet::new();
        matched.set(10);
        matched.set(11);
        matched.set(12);
        let nodes = [
            ExprNode::Sig(10),
            ExprNode::Sig(11),
            ExprNode::Sig(12),
            ExprNode::AllOf { count: 3 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_one_missing() {
        let mut matched = SigMatchSet::new();
        matched.set(10);
        matched.set(12);
        // sig 11 not matched
        let nodes = [
            ExprNode::Sig(10),
            ExprNode::Sig(11),
            ExprNode::Sig(12),
            ExprNode::AllOf { count: 3 },
        ];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_none_match() {
        let matched = SigMatchSet::new();
        let nodes = [
            ExprNode::Sig(10),
            ExprNode::Sig(11),
            ExprNode::AllOf { count: 2 },
        ];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_single_child_match() {
        let mut matched = SigMatchSet::new();
        matched.set(5);
        let nodes = [ExprNode::Sig(5), ExprNode::AllOf { count: 1 }];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_single_child_no_match() {
        let matched = SigMatchSet::new();
        let nodes = [ExprNode::Sig(5), ExprNode::AllOf { count: 1 }];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    // ── evaluate_rule: Not ──────────────────────────────────────────

    #[test]
    fn eval_not_inverts_true_to_false() {
        let mut matched = SigMatchSet::new();
        matched.set(3);
        let nodes = [ExprNode::Sig(3), ExprNode::Not];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_not_inverts_false_to_true() {
        let matched = SigMatchSet::new();
        let nodes = [ExprNode::Sig(3), ExprNode::Not];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_double_not_identity() {
        let mut matched = SigMatchSet::new();
        matched.set(3);
        let nodes = [ExprNode::Sig(3), ExprNode::Not, ExprNode::Not];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_not_of_anyof() {
        // not(anyOf(sig0, sig1)) — neither matched, so anyOf=false, not=true
        let matched = SigMatchSet::new();
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AnyOf { count: 2 },
            ExprNode::Not,
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_not_of_anyof_when_one_matches() {
        // not(anyOf(sig0, sig1)) — sig0 matched, so anyOf=true, not=false
        let mut matched = SigMatchSet::new();
        matched.set(0);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AnyOf { count: 2 },
            ExprNode::Not,
        ];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    // ── evaluate_rule: nested compositions ──────────────────────────

    #[test]
    fn eval_anyof_with_nested_allof() {
        // anyOf(sig0, allOf(sig1, sig2))
        // Encoding: [Sig(0), Sig(1), Sig(2), AllOf{2}, AnyOf{2}]
        //
        // Case: only sig1+sig2 match → allOf=true → anyOf=true
        let mut matched = SigMatchSet::new();
        matched.set(1);
        matched.set(2);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::AllOf { count: 2 },
            ExprNode::AnyOf { count: 2 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_anyof_with_nested_allof_only_first() {
        // anyOf(sig0, allOf(sig1, sig2))
        // Case: only sig0 matches → anyOf=true via sig0
        let mut matched = SigMatchSet::new();
        matched.set(0);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::AllOf { count: 2 },
            ExprNode::AnyOf { count: 2 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_anyof_with_nested_allof_partial() {
        // anyOf(sig0, allOf(sig1, sig2))
        // Case: only sig1 matches → allOf=false, sig0=false → anyOf=false
        let mut matched = SigMatchSet::new();
        matched.set(1);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::AllOf { count: 2 },
            ExprNode::AnyOf { count: 2 },
        ];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_with_nested_anyof() {
        // allOf(anyOf(sig0, sig1), sig2)
        // Encoding: [Sig(0), Sig(1), AnyOf{2}, Sig(2), AllOf{2}]
        let mut matched = SigMatchSet::new();
        matched.set(1);
        matched.set(2);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AnyOf { count: 2 },
            ExprNode::Sig(2),
            ExprNode::AllOf { count: 2 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_with_nested_anyof_missing_required() {
        // allOf(anyOf(sig0, sig1), sig2)
        // sig0 matches but sig2 doesn't → allOf=false
        let mut matched = SigMatchSet::new();
        matched.set(0);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AnyOf { count: 2 },
            ExprNode::Sig(2),
            ExprNode::AllOf { count: 2 },
        ];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_with_not_child() {
        // allOf(sig0, not(sig1)) — sig0 present, sig1 absent → true
        let mut matched = SigMatchSet::new();
        matched.set(0);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Not,
            ExprNode::AllOf { count: 2 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_with_not_child_fails() {
        // allOf(sig0, not(sig1)) — sig0 present, sig1 also present → false
        let mut matched = SigMatchSet::new();
        matched.set(0);
        matched.set(1);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Not,
            ExprNode::AllOf { count: 2 },
        ];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_deeply_nested() {
        // anyOf(allOf(sig0, not(sig1)), allOf(sig2, sig3))
        // Encoding:
        //   [Sig(0), Sig(1), Not, AllOf{2}, Sig(2), Sig(3), AllOf{2}, AnyOf{2}]
        //
        // Case: sig0=true, sig1=false → allOf(true, not(false)=true) = true → anyOf=true
        let mut matched = SigMatchSet::new();
        matched.set(0);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Not,
            ExprNode::AllOf { count: 2 },
            ExprNode::Sig(2),
            ExprNode::Sig(3),
            ExprNode::AllOf { count: 2 },
            ExprNode::AnyOf { count: 2 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_deeply_nested_second_branch() {
        // Same tree, but this time sig2+sig3 match instead
        let mut matched = SigMatchSet::new();
        matched.set(2);
        matched.set(3);
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Not,
            ExprNode::AllOf { count: 2 },
            ExprNode::Sig(2),
            ExprNode::Sig(3),
            ExprNode::AllOf { count: 2 },
            ExprNode::AnyOf { count: 2 },
        ];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_deeply_nested_neither_branch() {
        // Same tree, but sig1=true blocks first branch, sig3 missing blocks second
        let mut matched = SigMatchSet::new();
        matched.set(0);
        matched.set(1); // blocks not(sig1)
        matched.set(2); // sig3 missing
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Not,
            ExprNode::AllOf { count: 2 },
            ExprNode::Sig(2),
            ExprNode::Sig(3),
            ExprNode::AllOf { count: 2 },
            ExprNode::AnyOf { count: 2 },
        ];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    // ── evaluate_rule: edge cases ───────────────────────────────────

    #[test]
    fn eval_empty_expression_returns_false() {
        let matched = SigMatchSet::new();
        assert!(!evaluate_rule(&[], &matched));
    }

    #[test]
    fn eval_anyof_count_zero_leaves_stack_unchanged() {
        // AnyOf{count:0} with no prior stack items → should push false (vacuous OR)
        // But our stack has nothing, count=0 means start==stack.len(), result = .any() on empty = false
        let matched = SigMatchSet::new();
        let nodes = [ExprNode::AnyOf { count: 0 }];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_allof_count_zero_produces_true() {
        // AllOf{count:0} — vacuous truth
        let matched = SigMatchSet::new();
        let nodes = [ExprNode::AllOf { count: 0 }];
        assert!(evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_stack_underflow_not_returns_false() {
        // Not with empty stack — should return false (underflow guard)
        let matched = SigMatchSet::new();
        let nodes = [ExprNode::Not];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_stack_underflow_anyof_returns_false() {
        // AnyOf{count:3} with only 1 item on stack
        let matched = SigMatchSet::new();
        let nodes = [ExprNode::Sig(0), ExprNode::AnyOf { count: 3 }];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    #[test]
    fn eval_multiple_values_on_stack_returns_false() {
        // Two Sig leaves with no combining operator — stack has 2 items, not 1
        let mut matched = SigMatchSet::new();
        matched.set(0);
        matched.set(1);
        let nodes = [ExprNode::Sig(0), ExprNode::Sig(1)];
        assert!(!evaluate_rule(&nodes, &matched));
    }

    // ── evaluate_rule: real-world rule patterns ─────────────────────

    #[test]
    fn eval_flock_safety_pattern() {
        // Simulates: anyOf(sig0..sig7, allOf(sig8, sig6))
        // This is the Flock Safety Camera rule shape from the schema example
        let nodes = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::Sig(2),
            ExprNode::Sig(3),
            ExprNode::Sig(4),
            ExprNode::Sig(5),
            ExprNode::Sig(6),
            ExprNode::Sig(7),
            ExprNode::Sig(8),
            ExprNode::Sig(6), // sig6 referenced again
            ExprNode::AllOf { count: 2 },
            ExprNode::AnyOf { count: 9 },
        ];

        // Case 1: only sig0 matched → true (anyOf via sig0)
        let mut m = SigMatchSet::new();
        m.set(0);
        assert!(evaluate_rule(&nodes, &m));

        // Case 2: only sig8+sig6 → true (via allOf)
        let mut m = SigMatchSet::new();
        m.set(8);
        m.set(6);
        assert!(evaluate_rule(&nodes, &m));

        // Case 3: only sig8 → false (allOf needs sig6 too, and sig8 alone isn't in the flat anyOf)
        let mut m = SigMatchSet::new();
        m.set(8);
        assert!(!evaluate_rule(&nodes, &m));

        // Case 4: none matched → false
        let m = SigMatchSet::new();
        assert!(!evaluate_rule(&nodes, &m));
    }

    #[test]
    fn eval_simple_anyof_like_raven() {
        // Raven: anyOf(uuid0, uuid1, uuid2, uuid3, uuid4)
        let nodes = [
            ExprNode::Sig(20),
            ExprNode::Sig(21),
            ExprNode::Sig(22),
            ExprNode::Sig(23),
            ExprNode::Sig(24),
            ExprNode::AnyOf { count: 5 },
        ];

        let mut m = SigMatchSet::new();
        m.set(22);
        assert!(evaluate_rule(&nodes, &m));

        let m = SigMatchSet::new();
        assert!(!evaluate_rule(&nodes, &m));
    }

    #[test]
    fn eval_single_sig_like_airtag() {
        // AirTag: just one sig reference
        let nodes = [ExprNode::Sig(100)];

        let mut m = SigMatchSet::new();
        m.set(100);
        assert!(evaluate_rule(&nodes, &m));

        let m = SigMatchSet::new();
        assert!(!evaluate_rule(&nodes, &m));
    }

    #[test]
    fn eval_two_branch_anyof_like_flipper() {
        // Flipper Zero: anyOf(white, black)
        let nodes = [
            ExprNode::Sig(50),
            ExprNode::Sig(51),
            ExprNode::AnyOf { count: 2 },
        ];

        let mut m = SigMatchSet::new();
        m.set(50);
        assert!(evaluate_rule(&nodes, &m));

        let mut m = SigMatchSet::new();
        m.set(51);
        assert!(evaluate_rule(&nodes, &m));

        let m = SigMatchSet::new();
        assert!(!evaluate_rule(&nodes, &m));
    }

    // ── evaluate_rules (multi-rule) ─────────────────────────────────

    #[test]
    fn evaluate_rules_no_match() {
        static NODES: &[ExprNode] = &[
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AnyOf { count: 2 },
        ];
        static RULES: &[Rule] = &[Rule {
            name: "Rule A",
            expr_start: 0,
            expr_len: 3,
        }];
        let db = RuleDb {
            nodes: NODES,
            rules: RULES,
        };

        let matched = SigMatchSet::new();
        let result = evaluate_rules(&db, &matched);
        assert!(result.is_empty());
    }

    #[test]
    fn evaluate_rules_one_of_two_matches() {
        // Rule A: anyOf(sig0, sig1)
        // Rule B: sig2
        static NODES: &[ExprNode] = &[
            // Rule A: indices 0..3
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AnyOf { count: 2 },
            // Rule B: index 3
            ExprNode::Sig(2),
        ];
        static RULES: &[Rule] = &[
            Rule {
                name: "Rule A",
                expr_start: 0,
                expr_len: 3,
            },
            Rule {
                name: "Rule B",
                expr_start: 3,
                expr_len: 1,
            },
        ];
        let db = RuleDb {
            nodes: NODES,
            rules: RULES,
        };

        // Only sig0 matched → Rule A fires, Rule B doesn't
        let mut matched = SigMatchSet::new();
        matched.set(0);
        let result = evaluate_rules(&db, &matched);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], 0); // Rule A index
        assert_eq!(db.rules[result[0] as usize].name, "Rule A");
    }

    #[test]
    fn evaluate_rules_both_match() {
        static NODES: &[ExprNode] = &[
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AnyOf { count: 2 },
            ExprNode::Sig(2),
        ];
        static RULES: &[Rule] = &[
            Rule {
                name: "Rule A",
                expr_start: 0,
                expr_len: 3,
            },
            Rule {
                name: "Rule B",
                expr_start: 3,
                expr_len: 1,
            },
        ];
        let db = RuleDb {
            nodes: NODES,
            rules: RULES,
        };

        let mut matched = SigMatchSet::new();
        matched.set(0);
        matched.set(2);
        let result = evaluate_rules(&db, &matched);
        assert_eq!(result.len(), 2);
        assert_eq!(db.rules[result[0] as usize].name, "Rule A");
        assert_eq!(db.rules[result[1] as usize].name, "Rule B");
    }

    #[test]
    fn evaluate_rules_max_four_returned() {
        // 5 rules, all match — only first 4 returned
        static NODES: &[ExprNode] = &[
            ExprNode::Sig(0),
            ExprNode::Sig(0),
            ExprNode::Sig(0),
            ExprNode::Sig(0),
            ExprNode::Sig(0),
        ];
        static RULES: &[Rule] = &[
            Rule {
                name: "R0",
                expr_start: 0,
                expr_len: 1,
            },
            Rule {
                name: "R1",
                expr_start: 1,
                expr_len: 1,
            },
            Rule {
                name: "R2",
                expr_start: 2,
                expr_len: 1,
            },
            Rule {
                name: "R3",
                expr_start: 3,
                expr_len: 1,
            },
            Rule {
                name: "R4",
                expr_start: 4,
                expr_len: 1,
            },
        ];
        let db = RuleDb {
            nodes: NODES,
            rules: RULES,
        };

        let mut matched = SigMatchSet::new();
        matched.set(0);
        let result = evaluate_rules(&db, &matched);
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn evaluate_rules_invalid_range_skipped() {
        // Rule with expr_start+expr_len beyond nodes — should be skipped
        static NODES: &[ExprNode] = &[ExprNode::Sig(0)];
        static RULES: &[Rule] = &[Rule {
            name: "Bad rule",
            expr_start: 0,
            expr_len: 10, // way beyond nodes length
        }];
        let db = RuleDb {
            nodes: NODES,
            rules: RULES,
        };

        let mut matched = SigMatchSet::new();
        matched.set(0);
        let result = evaluate_rules(&db, &matched);
        assert!(result.is_empty());
    }

    // ── Stress / property-like tests ────────────────────────────────

    #[test]
    fn eval_anyof_is_commutative() {
        // anyOf(sig0, sig1) should equal anyOf(sig1, sig0) for any input
        let orders = [
            [
                ExprNode::Sig(0),
                ExprNode::Sig(1),
                ExprNode::AnyOf { count: 2 },
            ],
            [
                ExprNode::Sig(1),
                ExprNode::Sig(0),
                ExprNode::AnyOf { count: 2 },
            ],
        ];

        for sig_to_set in [None, Some(0u16), Some(1), Some(99)] {
            let mut m = SigMatchSet::new();
            if let Some(s) = sig_to_set {
                m.set(s);
            }
            let r0 = evaluate_rule(&orders[0], &m);
            let r1 = evaluate_rule(&orders[1], &m);
            assert_eq!(r0, r1, "AnyOf should be commutative for sig={sig_to_set:?}");
        }
    }

    #[test]
    fn eval_allof_is_commutative() {
        let orders = [
            [
                ExprNode::Sig(0),
                ExprNode::Sig(1),
                ExprNode::AllOf { count: 2 },
            ],
            [
                ExprNode::Sig(1),
                ExprNode::Sig(0),
                ExprNode::AllOf { count: 2 },
            ],
        ];

        for sigs in [vec![], vec![0], vec![1], vec![0, 1]] {
            let mut m = SigMatchSet::new();
            for s in &sigs {
                m.set(*s);
            }
            let r0 = evaluate_rule(&orders[0], &m);
            let r1 = evaluate_rule(&orders[1], &m);
            assert_eq!(r0, r1, "AllOf should be commutative for sigs={sigs:?}");
        }
    }

    #[test]
    fn eval_de_morgans_law_not_anyof() {
        // De Morgan's: not(anyOf(a, b)) == allOf(not(a), not(b))
        let lhs = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AnyOf { count: 2 },
            ExprNode::Not,
        ];
        let rhs = [
            ExprNode::Sig(0),
            ExprNode::Not,
            ExprNode::Sig(1),
            ExprNode::Not,
            ExprNode::AllOf { count: 2 },
        ];

        for sigs in [vec![], vec![0], vec![1], vec![0, 1]] {
            let mut m = SigMatchSet::new();
            for s in &sigs {
                m.set(*s);
            }
            assert_eq!(
                evaluate_rule(&lhs, &m),
                evaluate_rule(&rhs, &m),
                "De Morgan's law violated for sigs={sigs:?}"
            );
        }
    }

    #[test]
    fn eval_de_morgans_law_not_allof() {
        // De Morgan's: not(allOf(a, b)) == anyOf(not(a), not(b))
        let lhs = [
            ExprNode::Sig(0),
            ExprNode::Sig(1),
            ExprNode::AllOf { count: 2 },
            ExprNode::Not,
        ];
        let rhs = [
            ExprNode::Sig(0),
            ExprNode::Not,
            ExprNode::Sig(1),
            ExprNode::Not,
            ExprNode::AnyOf { count: 2 },
        ];

        for sigs in [vec![], vec![0], vec![1], vec![0, 1]] {
            let mut m = SigMatchSet::new();
            for s in &sigs {
                m.set(*s);
            }
            assert_eq!(
                evaluate_rule(&lhs, &m),
                evaluate_rule(&rhs, &m),
                "De Morgan's law violated for sigs={sigs:?}"
            );
        }
    }
}
