use super::*;
use crate::parse_ts_timestamp;

#[test]
fn routes_codex_auto_review_through_models_the_embedded_pricing_knows() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = crate::PricingMap::load_embedded();

    for fallback in CODEX_AUTO_REVIEW_FALLBACKS {
        assert!(
            pricing
                .resolve_at(
                    None,
                    &[(fallback.model, ccusage_core::pricing::MatchKind::Exact)],
                    None
                )
                .is_some(),
            "{} has no embedded pricing",
            fallback.model
        );
    }
    assert!(
        !CODEX_AUTO_REVIEW_FALLBACKS
            .iter()
            .any(|fallback| fallback.model == "gpt-5.5")
    );
    for (timestamp, expected) in [
        ("2025-01-01T00:00:00Z", "gpt-5"),
        ("2025-09-15T00:00:00Z", "gpt-5-codex"),
        ("2026-07-29T23:59:59Z", "gpt-5.4"),
        ("2026-07-30T00:00:00Z", "gpt-5.6-luna"),
        ("2026-07-30T00:30:00+01:00", "gpt-5.4"),
    ] {
        assert_eq!(
            codex_log_model_fallback(
                CODEX_AUTO_REVIEW_MODEL,
                parse_ts_timestamp(timestamp).unwrap()
            ),
            Some(expected),
            "{timestamp}"
        );
    }
}
