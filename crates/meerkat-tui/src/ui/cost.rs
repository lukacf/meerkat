//! Session cost accounting (pure): token totals per model and their price.
//!
//! The UI adds every `TurnCompleted` usage to the totals of the model the
//! provider accounted it to (the identity `TurnUsage` carries; the status
//! bar's model when it names none), every classifier call to the classifier
//! model, and prices the totals with a [`PriceTable`] whenever the
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

/// Token totals of a session by model, in first-use order.
///
/// `known` is false when the totals were derived from the session's
/// cumulative usage (resuming a session recorded before cost tracking): the
/// models are then a guess and the readout is marked approximate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCost {
    /// Totals by model id.
    pub per_model: IndexMap<String, UsageTotals>,
    /// Whether every total is attributed to the model that produced it.
    pub known: bool,
}

/// What [`crate::ui::Effect::CostSnapshot`] carries and the sidecar stores.
pub type SessionCostSnapshot = SessionCost;

impl Default for SessionCost {
    fn default() -> Self {
        Self {
            per_model: IndexMap::new(),
            known: true,
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
    /// `model` and marked approximate (`known = false`). Zero usage gives an
    /// empty, known cost.
    #[must_use]
    pub fn derived(model: &str, usage: &Usage) -> Self {
        let totals = UsageTotals::from_usage(usage);
        if totals.is_zero() {
            return Self::default();
        }
        Self {
            per_model: IndexMap::from([(model.to_string(), totals)]),
            known: false,
        }
    }

    /// Add `totals` to `model`.
    pub fn add(&mut self, model: &str, totals: &UsageTotals) {
        if totals.is_zero() {
            return;
        }
        match self.per_model.get_mut(model) {
            Some(existing) => existing.add(totals),
            None => {
                self.per_model.insert(model.to_string(), *totals);
            }
        }
    }

    /// Add one model call's usage to `model`.
    pub fn record(&mut self, model: &str, usage: &Usage) {
        self.add(model, &UsageTotals::from_usage(usage));
    }

    /// Count `tokens` as output of `model` (compaction summaries, where only
    /// the summary size is reported).
    pub fn record_output(&mut self, model: &str, tokens: u64) {
        self.add(
            model,
            &UsageTotals {
                output: tokens,
                ..UsageTotals::default()
            },
        );
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
        let mut approx = !self.known;
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
        let total = match self.readout(prices) {
            CostReadout::Usd { usd, approx } => {
                format!("{}{}", if approx { "~" } else { "" }, fmt_usd(usd))
            }
            CostReadout::Unknown | CostReadout::Unused => "$?".to_string(),
        };
        let _ = write!(out, "\n  total: {total}");
        if !self.known {
            out.push_str(
                "\n  (totals before this run are derived from the session's usage and attributed to one model)",
            );
        }
        out
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
        cost.record_output("b", 50_000);
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
        assert!(!derived.known);
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
