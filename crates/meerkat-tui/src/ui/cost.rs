//! Session cost accounting (pure): token totals per model and their price.
//!
//! The UI adds every `TurnCompleted` usage to the totals of the model the
//! provider accounted it to (the identity `TurnUsage` carries; the status
//! bar's model when it names none), every classifier call to the classifier
//! model, every helper's `TurnCompleted` usage to its model *and* to the
//! helper's own totals ([`SessionCost::helpers`], the `/cost` breakdown per
//! helper), and prices the totals with a [`PriceTable`] whenever the
//! footer is drawn or `/cost` is asked for. Meerkat reports
//! `input_tokens` without the prompt-cache tokens (Anthropic semantics; the
//! other clients report no cache tokens), so the cost of one model is
//!
//! ```text
//! ( input       × price.input
//! + cache_read  × (price.cached_input or price.input)
//! + cache_write × (price.cache_write  or price.input)
//! + output      × price.output ) / 1M
//! ```

use std::fmt::Write as _;

use indexmap::IndexMap;
use meerkat_core::Usage;
use serde::{Deserialize, Serialize};

use crate::pricing::{ModelPrice, PriceSource, PriceTable, TOKENS_PER_UNIT};

/// Cumulative tokens of one model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageTotals {
    /// Uncached input tokens.
    #[serde(default)]
    pub input: u64,
    /// Output tokens.
    #[serde(default)]
    pub output: u64,
    /// Input tokens read from the prompt cache.
    #[serde(default)]
    pub cache_read: u64,
    /// Input tokens written to the prompt cache.
    #[serde(default)]
    pub cache_write: u64,
}

impl UsageTotals {
    /// Totals of one [`Usage`].
    #[must_use]
    pub fn from_usage(usage: &Usage) -> Self {
        Self {
            input: usage.input_tokens,
            output: usage.output_tokens,
            cache_read: usage.cache_read_tokens.unwrap_or(0),
            cache_write: usage.cache_creation_tokens.unwrap_or(0),
        }
    }

    /// Add `other`.
    pub fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
    }

    /// Whether no token was counted.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }

    /// Cost in USD at `price` (see the module docs).
    #[must_use]
    pub fn cost(&self, price: &ModelPrice) -> f64 {
        let cached = price.cached_input.unwrap_or(price.input);
        let write = price.cache_write.unwrap_or(price.input);
        (self.input as f64 * price.input
            + self.cache_read as f64 * cached
            + self.cache_write as f64 * write
            + self.output as f64 * price.output)
            / TOKENS_PER_UNIT
    }
}

/// Token totals by model id, in first-use order.
pub type ModelTotals = IndexMap<String, UsageTotals>;

/// How much the totals of a [`SessionCost`] can be trusted.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CostConfidence {
    /// Every model call was counted under the model that produced it.
    #[default]
    Exact,
    /// Some totals are estimates: derived from the session's cumulative
    /// usage (a session recorded before cost tracking, or whose authoritative
    /// usage exceeds the recorded totals), or a compaction whose input was
    /// only estimated. The footer shows `~`.
    Approximate,
    /// Some usage was not observed at all (a helper whose events could not
    /// be followed): the totals are a lower bound. The footer shows `~`.
    Unmeasured,
}

impl CostConfidence {
    /// The weaker of `self` and `other`.
    #[must_use]
    pub fn and(self, other: Self) -> Self {
        self.max(other)
    }
}

/// Token totals of a session by model, in first-use order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCost {
    /// Totals by model id (the session's own calls and its helpers').
    pub per_model: ModelTotals,
    /// How far the totals can be trusted.
    #[serde(default)]
    pub confidence: CostConfidence,
    /// The helpers' share of `per_model`, by helper label.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub helpers: IndexMap<String, ModelTotals>,
}

/// What [`crate::ui::Effect::CostSnapshot`] carries and the sidecar stores.
pub type SessionCostSnapshot = SessionCost;

impl Default for SessionCost {
    fn default() -> Self {
        Self {
            per_model: IndexMap::new(),
            confidence: CostConfidence::Exact,
            helpers: IndexMap::new(),
        }
    }
}

/// The footer's view of a [`SessionCost`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CostReadout {
    /// Nothing used yet: no segment.
    Unused,
    /// Tokens were used but no model is priced: `$?`.
    Unknown,
    /// The cost, `~` when approximate.
    Usd {
        /// US dollars.
        usd: f64,
        /// Some model is unpriced, the totals are derived, or a price is
        /// a stale estimate.
        approx: bool,
    },
}

impl SessionCost {
    /// Totals derived from a session's cumulative `usage`, attributed to
    /// `model` and marked [`CostConfidence::Approximate`]. Zero usage gives
    /// an empty, exact cost.
    #[must_use]
    pub fn derived(model: &str, usage: &Usage) -> Self {
        let totals = UsageTotals::from_usage(usage);
        if totals.is_zero() {
            return Self::default();
        }
        Self {
            per_model: IndexMap::from([(model.to_string(), totals)]),
            confidence: CostConfidence::Approximate,
            helpers: IndexMap::new(),
        }
    }

    /// Add `totals` to `model`.
    pub fn add(&mut self, model: &str, totals: &UsageTotals) {
        add_to(&mut self.per_model, model, totals);
    }

    /// Add one model call of the helper `label` (on `model`): to the
    /// session's totals and to the helper's own.
    pub fn record_helper(&mut self, label: &str, model: &str, usage: &Usage) {
        let totals = UsageTotals::from_usage(usage);
        if totals.is_zero() {
            return;
        }
        self.add(model, &totals);
        add_to(
            self.helpers.entry(label.to_string()).or_default(),
            model,
            &totals,
        );
    }

    /// Add one model call's usage to `model`.
    pub fn record(&mut self, model: &str, usage: &Usage) {
        self.add(model, &UsageTotals::from_usage(usage));
    }

    /// Count a compaction's summary call on `model`: `input` is the history
    /// estimate the compaction reported, `output` the summary size. The
    /// input is an estimate, so the totals become
    /// [`CostConfidence::Approximate`].
    pub fn record_compaction(&mut self, model: &str, input: u64, output: u64) {
        let totals = UsageTotals {
            input,
            output,
            ..UsageTotals::default()
        };
        if totals.is_zero() {
            return;
        }
        self.add(model, &totals);
        self.confidence = self.confidence.and(CostConfidence::Approximate);
    }

    /// Some of helper `label`'s usage could not be observed.
    pub fn mark_unmeasured(&mut self, label: &str) {
        self.helpers.entry(label.to_string()).or_default();
        self.confidence = self.confidence.and(CostConfidence::Unmeasured);
    }

    /// All input and output tokens counted, cache tokens included (as the
    /// session's `total_usage` counts them).
    #[must_use]
    pub fn token_sum(&self) -> u64 {
        self.per_model.values().fold(0_u64, |sum, t| {
            sum.saturating_add(t.input)
                .saturating_add(t.output)
                .saturating_add(t.cache_read)
                .saturating_add(t.cache_write)
        })
    }

    /// Reconcile the recorded totals with the session's authoritative
    /// cumulative `usage`: usage the record does not account for (calls
    /// another surface made, a compaction published after the TUI stopped
    /// following) makes the readout [`CostConfidence::Approximate`]; the
    /// missing tokens are attributed to `model`.
    pub fn reconcile(&mut self, model: &str, usage: &Usage) {
        let authoritative = UsageTotals::from_usage(usage);
        let recorded = self.per_model.values().fold(UsageTotals::default(), |mut sum, t| {
            sum.add(t);
            sum
        });
        let missing = UsageTotals {
            input: authoritative.input.saturating_sub(recorded.input),
            output: authoritative.output.saturating_sub(recorded.output),
            cache_read: authoritative.cache_read.saturating_sub(recorded.cache_read),
            cache_write: authoritative.cache_write.saturating_sub(recorded.cache_write),
        };
        if missing.is_zero() {
            return;
        }
        self.add(model, &missing);
        self.confidence = self.confidence.and(CostConfidence::Approximate);
    }

    /// Whether no token was counted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.per_model.values().all(UsageTotals::is_zero)
    }

    /// The cost of the priced models in USD, and whether every model was
    /// priced. Unpriced models are left out.
    #[must_use]
    pub fn cost_usd(&self, prices: &PriceTable) -> (f64, bool) {
        let mut usd = 0.0;
        let mut all = true;
        for (model, totals) in &self.per_model {
            match prices.lookup(model) {
                Some((price, _)) => usd += totals.cost(&price),
                None => all = false,
            }
        }
        (usd, all)
    }

    /// The footer readout.
    #[must_use]
    pub fn readout(&self, prices: &PriceTable) -> CostReadout {
        if self.is_empty() {
            return CostReadout::Unused;
        }
        let mut usd = 0.0;
        let mut priced = 0_usize;
        let mut approx = self.confidence != CostConfidence::Exact;
        for (model, totals) in self.per_model.iter().filter(|(_, t)| !t.is_zero()) {
            match prices.lookup(model) {
                Some((price, source)) => {
                    usd += totals.cost(&price);
                    priced += 1;
                    approx |= source.is_estimate();
                }
                None => approx = true,
            }
        }
        if priced == 0 {
            CostReadout::Unknown
        } else {
            CostReadout::Usd { usd, approx }
        }
    }

    /// The `/cost` breakdown: tokens and USD per model with the price source.
    #[must_use]
    pub fn breakdown(&self, prices: &PriceTable) -> String {
        if self.is_empty() {
            return "cost: no tokens used in this session yet".to_string();
        }
        let mut out = String::from("Session cost by model:");
        for (model, totals) in self.per_model.iter().filter(|(_, t)| !t.is_zero()) {
            let usd = match prices.lookup(model) {
                Some((price, source)) => {
                    format!(
                        "{} ({})",
                        fmt_usd(totals.cost(&price)),
                        source_label(source)
                    )
                }
                None => "$? (no price; set [pricing.\"<id>\"] in tui.toml)".to_string(),
            };
            let _ = write!(
                out,
                "\n  {model}: in {} · out {} · cache read {} · cache write {} · {usd}",
                totals.input, totals.output, totals.cache_read, totals.cache_write
            );
        }
        self.write_helpers(&mut out, prices);
        let total = match self.readout(prices) {
            CostReadout::Usd { usd, approx } => {
                format!("{}{}", if approx { "~" } else { "" }, fmt_usd(usd))
            }
            CostReadout::Unknown | CostReadout::Unused => "$?".to_string(),
        };
        let _ = write!(out, "\n  total: {total}");
        match self.confidence {
            CostConfidence::Exact => {}
            CostConfidence::Approximate => out.push_str(
                "\n  (approximate: part of the totals is derived from the session's recorded usage or a compaction estimate)",
            ),
            CostConfidence::Unmeasured => out.push_str(
                "\n  (a lower bound: some helper usage could not be observed)",
            ),
        }
        out
    }
}

impl SessionCost {
    /// The helpers' lines of the `/cost` breakdown (their tokens are
    /// already in the per-model lines above them).
    fn write_helpers(&self, out: &mut String, prices: &PriceTable) {
        let helpers: Vec<_> = self
            .helpers
            .iter()
            .flat_map(|(label, models)| models.iter().map(move |(model, t)| (label, model, t)))
            .filter(|(_, _, totals)| !totals.is_zero())
            .collect();
        if helpers.is_empty() {
            return;
        }
        out.push_str("\n  of which helpers:");
        for (label, model, totals) in helpers {
            let usd = prices.lookup(model).map_or_else(
                || "$?".to_string(),
                |(price, _)| fmt_usd(totals.cost(&price)),
            );
            let _ = write!(
                out,
                "\n    {label} ({model}): in {} · out {} · cache read {} · cache write {} · {usd}",
                totals.input, totals.output, totals.cache_read, totals.cache_write
            );
        }
    }
}

/// Add `totals` to `model` in `per_model`.
fn add_to(per_model: &mut ModelTotals, model: &str, totals: &UsageTotals) {
    if totals.is_zero() {
        return;
    }
    match per_model.get_mut(model) {
        Some(existing) => existing.add(totals),
        None => {
            per_model.insert(model.to_string(), *totals);
        }
    }
}

fn source_label(source: PriceSource) -> String {
    format!("price: {source}")
}

/// Dollars with two decimals; `$12.3k` from 10 000 on.
#[must_use]
pub fn fmt_usd(usd: f64) -> String {
    if usd >= 10_000.0 {
        format!("${:.1}k", usd / 1_000.0)
    } else {
        format!("${usd:.2}")
    }
}

/// The footer segment text: `$0.42`, `~$0.42`, `$?`; `None` before any use.
#[must_use]
pub fn cost_segment_text(cost: &SessionCost, prices: &PriceTable) -> Option<String> {
    match cost.readout(prices) {
        CostReadout::Unused => None,
        CostReadout::Unknown => Some("$?".to_string()),
        CostReadout::Usd { usd, approx } => {
            Some(format!("{}{}", if approx { "~" } else { "" }, fmt_usd(usd)))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::approval::rules::PricingSection;
    use std::collections::BTreeMap;

    fn price(input: f64, output: f64, cached: Option<f64>, write: Option<f64>) -> PricingSection {
        PricingSection {
            input: Some(input),
            output: Some(output),
            cached_input: cached,
            cache_write: write,
        }
    }

    fn table(entries: &[(&str, PricingSection)]) -> PriceTable {
        PriceTable::new(
            entries
                .iter()
                .map(|(id, p)| ((*id).to_string(), *p))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn usage(input: u64, output: u64, read: Option<u64>, write: Option<u64>) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_tokens: write,
            cache_read_tokens: read,
            ..Usage::default()
        }
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn arithmetic_with_cache_tokens() {
        let prices = table(&[("opus", price(15.0, 75.0, Some(1.5), Some(18.75)))]);
        let mut cost = SessionCost::default();
        cost.record(
            "opus",
            &usage(1_000_000, 100_000, Some(2_000_000), Some(400_000)),
        );
        // 1M × 15 + 2M × 1.5 + 0.4M × 18.75 + 0.1M × 75 = 15 + 3 + 7.5 + 7.5
        close(cost.cost_usd(&prices).0, 33.0);
        assert!(cost.cost_usd(&prices).1);
        // Missing cache prices fall back to the input price.
        let prices = table(&[("opus", price(15.0, 75.0, None, None))]);
        // 1M × 15 + 2M × 15 + 0.4M × 15 + 0.1M × 75 = 15 + 30 + 6 + 7.5
        close(cost.cost_usd(&prices).0, 58.5);
    }

    #[test]
    fn multi_model_accumulation_in_first_use_order() {
        let prices = table(&[
            ("a", price(1.0, 2.0, None, None)),
            ("b", price(10.0, 20.0, None, None)),
        ]);
        let mut cost = SessionCost::default();
        cost.record("a", &usage(500_000, 0, None, None));
        cost.record("b", &usage(100_000, 50_000, None, None));
        cost.record("a", &usage(500_000, 1_000_000, None, None));
        cost.record("b", &usage(0, 50_000, None, None));
        cost.record("a", &usage(0, 0, None, None));
        let order: Vec<&str> = cost.per_model.keys().map(String::as_str).collect();
        assert_eq!(order, vec!["a", "b"]);
        assert_eq!(cost.per_model["a"].input, 1_000_000);
        assert_eq!(cost.per_model["b"].output, 100_000);
        // a: 1 + 2 = 3; b: 1 + 2 = 3
        close(cost.cost_usd(&prices).0, 6.0);
        assert_eq!(cost_segment_text(&cost, &prices).as_deref(), Some("$6.00"));
    }

    #[test]
    fn unknown_model_is_excluded_and_marks_approx() {
        let prices = table(&[("a", price(1.0, 2.0, None, None))]);
        let mut cost = SessionCost::default();
        cost.record("a", &usage(420_000, 0, None, None));
        cost.record("mystery", &usage(1_000_000, 1_000_000, None, None));
        let (usd, all) = cost.cost_usd(&prices);
        close(usd, 0.42);
        assert!(!all);
        assert_eq!(cost_segment_text(&cost, &prices).as_deref(), Some("~$0.42"));
    }

    #[test]
    fn nothing_priced_is_unknown_and_nothing_used_is_hidden() {
        let prices = PriceTable::default();
        let mut cost = SessionCost::default();
        assert_eq!(cost.readout(&prices), CostReadout::Unused);
        assert_eq!(cost_segment_text(&cost, &prices), None);
        cost.record("mystery", &usage(10, 10, None, None));
        assert_eq!(cost.readout(&prices), CostReadout::Unknown);
        assert_eq!(cost_segment_text(&cost, &prices).as_deref(), Some("$?"));
    }

    #[test]
    fn derived_totals_are_approximate() {
        let prices = table(&[("a", price(1.0, 1.0, None, None))]);
        let derived = SessionCost::derived("a", &usage(1_000_000, 0, Some(0), None));
        assert_eq!(derived.confidence, CostConfidence::Approximate);
        assert_eq!(
            cost_segment_text(&derived, &prices).as_deref(),
            Some("~$1.00")
        );
        assert_eq!(
            SessionCost::derived("a", &Usage::default()),
            SessionCost::default()
        );
    }

    #[test]
    fn helper_usage_rolls_up_and_is_broken_down() {
        let prices = table(&[
            ("a", price(1.0, 2.0, None, None)),
            ("b", price(10.0, 20.0, None, None)),
        ]);
        let mut cost = SessionCost::default();
        cost.record("a", &usage(1_000_000, 0, None, None));
        cost.record_helper("scout", "a", &usage(500_000, 0, None, None));
        cost.record_helper("scout", "b", &usage(100_000, 0, None, None));
        cost.record_helper("idle", "b", &usage(0, 0, None, None));
        // The footer total includes the helpers: 1.5 × 1 + 0.1 × 10.
        close(cost.cost_usd(&prices).0, 2.5);
        assert_eq!(cost.per_model["a"].input, 1_500_000);
        assert_eq!(cost.helpers["scout"]["b"].input, 100_000);
        assert!(!cost.helpers.contains_key("idle"));
        let text = cost.breakdown(&prices);
        assert!(text.contains("of which helpers:"), "{text}");
        assert!(text.contains("scout (a): in 500000"), "{text}");
        assert!(
            text.contains("scout (b): in 100000 · out 0 · cache read 0 · cache write 0 · $1.00"),
            "{text}"
        );
        assert!(text.contains("total: $2.50"), "{text}");
        // No helpers, no helper section.
        let mut plain = SessionCost::default();
        plain.record("a", &usage(1, 1, None, None));
        assert!(!plain.breakdown(&prices).contains("helpers"));
    }

    #[test]
    fn confidence_follows_reconciliation_compaction_and_helpers() {
        let prices = table(&[("a", price(1.0, 1.0, None, None))]);
        let mut cost = SessionCost::default();
        cost.record("a", &usage(1_000_000, 0, None, None));
        // The session's usage matches the record: still exact.
        cost.reconcile("a", &usage(1_000_000, 0, None, None));
        assert_eq!(cost.confidence, CostConfidence::Exact);
        assert_eq!(cost_segment_text(&cost, &prices).as_deref(), Some("$1.00"));
        // The session's usage exceeds the record: the gap is attributed
        // and the readout is approximate.
        cost.reconcile("a", &usage(2_000_000, 0, None, None));
        assert_eq!(cost.confidence, CostConfidence::Approximate);
        assert_eq!(cost.per_model["a"].input, 2_000_000);
        assert_eq!(cost_segment_text(&cost, &prices).as_deref(), Some("~$2.00"));
        // A compaction counts its estimated input and its summary.
        let mut compacted = SessionCost::default();
        compacted.record_compaction("a", 1_000_000, 10);
        assert_eq!(compacted.per_model["a"].input, 1_000_000);
        assert_eq!(compacted.per_model["a"].output, 10);
        assert_eq!(compacted.confidence, CostConfidence::Approximate);
        // A helper that could not be followed makes it a lower bound, which
        // a later approximation never upgrades.
        compacted.mark_unmeasured("scout");
        compacted.reconcile("a", &usage(9_000_000, 0, None, None));
        assert_eq!(compacted.confidence, CostConfidence::Unmeasured);
        assert!(compacted.breakdown(&prices).contains("lower bound"));
    }

    #[test]
    fn usd_formatting() {
        assert_eq!(fmt_usd(0.0), "$0.00");
        assert_eq!(fmt_usd(0.424), "$0.42");
        assert_eq!(fmt_usd(9_999.994), "$9999.99");
        assert_eq!(fmt_usd(12_345.0), "$12.3k");
    }

    #[test]
    fn breakdown_lists_tokens_usd_and_source() {
        let prices = table(&[("a", price(1.0, 2.0, None, None))]);
        let mut cost = SessionCost::default();
        cost.record("a", &usage(1_000_000, 500_000, Some(7), Some(3)));
        cost.record("mystery", &usage(1, 1, None, None));
        let text = cost.breakdown(&prices);
        assert!(
            text.contains("a: in 1000000 · out 500000 · cache read 7 · cache write 3"),
            "{text}"
        );
        assert!(text.contains("(price: tui.toml)"), "{text}");
        assert!(text.contains("mystery: in 1 · out 1"), "{text}");
        assert!(text.contains("$? (no price"), "{text}");
        assert!(text.contains("total: ~$2.00"), "{text}");
        assert!(
            SessionCost::default()
                .breakdown(&prices)
                .contains("no tokens used")
        );
    }
}
