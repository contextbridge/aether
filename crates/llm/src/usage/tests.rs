use super::*;

#[test]
fn accounting_units_are_zero_cost_transparent_values() {
    assert_eq!(std::mem::size_of::<Tokens>(), std::mem::size_of::<u64>());
    assert_eq!(std::mem::size_of::<Usd>(), std::mem::size_of::<f64>());
    assert_eq!(serde_json::to_value(Tokens::new(42)).unwrap(), serde_json::json!(42));
    assert_eq!(serde_json::to_value(Usd::new(1.25)).unwrap(), serde_json::json!(1.25));
}

#[test]
fn adding_samples_keeps_unreported_dimensions_unreported() {
    let anthropic = TokenUsage {
        cache_read_tokens: Some(Tokens::new(200)),
        cache_creation_tokens: Some(Tokens::new(50)),
        ..TokenUsage::new(500, 100)
    };
    let openai = TokenUsage {
        cache_read_tokens: Some(Tokens::new(300)),
        reasoning_tokens: Some(Tokens::new(30)),
        ..TokenUsage::new(600, 40)
    };
    let total = anthropic + openai;
    assert_eq!(total.input_tokens.get(), 1_100);
    assert_eq!(total.output_tokens.get(), 140);
    assert_eq!(total.cache_read_tokens.map(Tokens::get), Some(500));
    assert_eq!(total.cache_creation_tokens.map(Tokens::get), Some(50));
    assert_eq!(total.reasoning_tokens.map(Tokens::get), Some(30));
    assert_eq!(total.input_audio_tokens, None);
    assert_eq!(total.total_tokens().get(), 1_240);
    assert_eq!(TokenUsage::default() + TokenUsage::default(), TokenUsage::default());
}

#[test]
fn sums_saturate_instead_of_wrapping() {
    let huge = TokenUsage { input_tokens: Tokens::new(u64::MAX), ..TokenUsage::default() };
    assert_eq!((huge + TokenUsage::new(1, 0)).input_tokens.get(), u64::MAX);
}

#[test]
fn is_zero_ignores_which_dimensions_were_reported() {
    assert!(TokenUsage { cache_read_tokens: Some(Tokens::ZERO), ..TokenUsage::default() }.is_zero());
    assert!(!TokenUsage::new(0, 1).is_zero());
}

#[test]
fn estimates_ordinary_cost() {
    let cost = pricing().estimate_cost(TokenUsage::new(1_000_000, 500_000));
    assert!((cost.total_usd.get() - 20.0).abs() < 1e-12);
}

#[test]
fn cached_tokens_are_priced_at_cache_rates_and_removed_from_the_input_pool() {
    let cost = pricing().estimate_cost(TokenUsage {
        input_tokens: Tokens::new(1_000_000),
        cache_read_tokens: Some(Tokens::new(200_000)),
        cache_creation_tokens: Some(Tokens::new(100_000)),
        ..TokenUsage::default()
    });
    assert!((cost.input_usd.get() - 7.0).abs() < 1e-12);
    assert!((cost.cache_read_usd.get() - 0.2).abs() < 1e-12);
    assert!((cost.cache_creation_usd.get() - 0.2).abs() < 1e-12);
    assert!((cost.total_usd.get() - 7.4).abs() < 1e-12);
}

#[test]
fn missing_cache_rates_fall_back_to_input_rate() {
    let pricing = ModelPricing { cache_read_per_million: None, cache_write_per_million: None, ..pricing() };
    let cost = pricing.estimate_cost(TokenUsage {
        input_tokens: Tokens::new(100),
        cache_read_tokens: Some(Tokens::new(100)),
        ..TokenUsage::default()
    });
    assert!((cost.total_usd.get() - 0.001).abs() < 1e-12);
}

#[test]
fn more_cached_than_input_tokens_saturates_the_input_pool() {
    let malformed = pricing().estimate_cost(TokenUsage {
        input_tokens: Tokens::new(10),
        cache_read_tokens: Some(Tokens::new(20)),
        ..TokenUsage::default()
    });
    assert!(malformed.input_usd.get().abs() < 1e-12);
    assert!((malformed.cache_read_usd.get() - 0.00002).abs() < 1e-12);
}

#[test]
fn totals_sum_priced_calls_and_count_unpriced_ones() {
    let mut totals = SessionUsageTotals::default();
    assert!(totals.is_fully_priced());

    totals.add(TokenUsage::new(1, 1), Some(UsageCost { total_usd: Usd::new(0.5), ..UsageCost::default() }));
    assert!(totals.is_fully_priced());
    assert!((totals.estimated_usd.get() - 0.5).abs() < 1e-12);

    totals.add(TokenUsage::new(2, 3), None);
    assert!(!totals.is_fully_priced());
    assert_eq!(totals.unpriced_calls, 1);
    assert_eq!(totals.tokens.input_tokens.get(), 3);
    assert_eq!(totals.tokens.output_tokens.get(), 4);

    totals.add(TokenUsage::new(1, 1), Some(UsageCost { total_usd: Usd::new(0.25), ..UsageCost::default() }));
    assert!((totals.estimated_usd.get() - 0.75).abs() < 1e-12);
    assert_eq!(totals.unpriced_calls, 1);
}

#[test]
fn zero_usage_without_pricing_does_not_count_as_unpriced() {
    let mut totals = SessionUsageTotals::default();
    totals.add(TokenUsage::default(), None);
    assert_eq!(totals, SessionUsageTotals::default());
}

#[test]
fn new_sources_have_fresh_ids_and_no_lineage() {
    let first = UsageSource::new("root");
    let second = UsageSource::new("root");
    assert_ne!(first.agent_id, second.agent_id);
    assert_eq!(first.parent_agent_id, None);
    assert_eq!(first.task_id, None);
    assert_eq!(first.agent_name, "root");
}

#[test]
fn model_identity_is_empty_without_a_catalog_model() {
    assert_eq!(ModelIdentity::of(None), ModelIdentity::default());
}

fn pricing() -> ModelPricing {
    ModelPricing {
        input_per_million: 10.0,
        output_per_million: 20.0,
        cache_read_per_million: Some(1.0),
        cache_write_per_million: Some(2.0),
    }
}
