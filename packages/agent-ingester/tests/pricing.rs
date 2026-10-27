use agent_common::types::TokenUsage;
use agent_ingester::pricing::Engine;

#[test]
fn model_prices_include_cached_usage_and_preserve_unknown_cost() {
    let engine = Engine::default();
    let usage = TokenUsage {
        input: 1000,
        output: 500,
        cache_read: 1000,
        cache_write: 100,
        reasoning: 300,
        ..Default::default()
    };
    let actual = engine.calculate(" CLAUDE-OPUS-5 ", &usage).unwrap();
    assert!((actual - 0.055875).abs() < 1e-12);
    assert_eq!(engine.calculate("unknown-model", &usage), None);
    assert_eq!(engine.calculate("", &usage), None);
    assert!(engine.calculate("claude-3-7-sonnet", &usage).is_some());
    assert!(
        engine
            .calculate("claude-haiku-4-5-20251001", &usage)
            .is_some()
    );
    assert!(
        engine
            .calculate("claude-haiku-4-5-20990101", &usage)
            .is_some()
    );
}

#[test]
fn ambiguous_prefixes_choose_specific_deterministic_rate() {
    let mut engine = Engine::from_json(br#"{"test":{"input_cost_per_token":1},"test-special":{"input_cost_per_token":2},"test-another":{"input_cost_per_token":3}}"#).unwrap();
    let usage = TokenUsage {
        input: 1,
        ..Default::default()
    };
    assert_eq!(engine.calculate("test-special-20260101", &usage), Some(2.0));
    assert_eq!(engine.calculate("TEST", &usage), Some(1.0));
    engine
        .merge_json(br#"{"test":{"input_cost_per_token":4}}"#)
        .unwrap();
    assert_eq!(engine.calculate("test", &usage), Some(4.0));
    assert!(engine.merge_json(b"invalid").is_err());
    assert_eq!(engine.calculate("test", &usage), Some(4.0));
}
