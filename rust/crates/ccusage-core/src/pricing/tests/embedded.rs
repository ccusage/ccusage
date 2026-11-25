use super::{
    MatchKind, PricingMap, build_time_models_dev_json, build_time_pricing_json,
    embedded_models_dev_pricing, layered,
};
use crate::{
    fast::{FxHashMap, FxHashSet},
    pricing::{
        Lookup, Name, PricingRows, data,
        keys::{is_qualifier_end, normalized_pricing_key},
        rows::{Index, RowMatch, vendor_lookup_id},
    },
};
use std::{sync::OnceLock, thread};

fn sorted_rows(rows: &PricingRows) -> Vec<String> {
    let mut rows: Vec<_> = rows
        .matches()
        .map(|found| {
            let row = found.row;
            format!(
                "{} {:?} {:?} {:?} {:?} {} {} {:?} {:?} {:?}",
                row.key,
                row.rates(),
                row.source,
                row.schedule,
                row.context_limit,
                row.exact_only,
                row.whole_request_tiers,
                row.scope,
                row.vendor_start,
                row.host,
            )
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn lookup_decodes_only_matching_embedded_rows() {
    use crate::pricing::table::{RecordOffsets, decode_pricing_table};

    crate::test_support::install_pricing_snapshots();
    let table = PricingMap::from_embedded_rows(decode_pricing_table(
        ccusage_pricing_snapshots::PRICING_TABLE,
        RecordOffsets::Absolute,
    ));
    let rows = &table.rows;
    assert_eq!(rows.decode_cache_counts(), (0, 0));

    let ready = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                let identity = OnceLock::new();
                let name = Name::new("gpt-5.5", &identity);
                ready.wait();
                let row = rows.exact(name, Lookup::Pricing, false).unwrap();
                assert_eq!(row.row.rates().input, 5e-6);
            });
        }
    });
    let (slots, decoded) = rows.decode_cache_counts();
    assert_eq!(decoded, 1);
    assert!(slots < rows.keys().count());
    assert!(table.find("no-such-model-for-lazy-pricing-test").is_none());
    assert_eq!(rows.decode_cache_counts(), (slots, 1));
}

#[test]
fn a_primary_hit_and_unlisted_synthetic_model_leave_the_secondary_table_cold() {
    use std::sync::Arc;

    use crate::pricing::embedded::{embedded_models_dev_index, embedded_pricing_table};

    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap {
        rows: Arc::clone(&embedded_pricing_table().rows),
        models_dev: Some(Arc::new(PricingRows::from_deferred(
            || panic!("the secondary catalogue should remain cold"),
            embedded_models_dev_index(),
        ))),
        ..PricingMap::default()
    };
    assert!(pricing.find("gpt-5.5").is_some());
    assert!(pricing.find("<synthetic>").is_none());
}

#[test]
fn unknown_override_keeps_embedded_catalogues_cold() {
    use std::sync::Arc;

    use crate::pricing::{
        embedded::embedded_models_dev_index,
        table::{RecordOffsets, decode_pricing_table},
    };

    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::from_embedded_rows(decode_pricing_table(
        ccusage_pricing_snapshots::PRICING_TABLE,
        RecordOffsets::Absolute,
    ));
    pricing.models_dev = Some(Arc::new(PricingRows::from_deferred(
        || panic!("an unknown override should leave the secondary catalogue cold"),
        embedded_models_dev_index(),
    )));
    let key = "brand-new-model".to_owned();
    let rates = ccusage_cli::PricingOverride {
        input_cost_per_token: Some(1e-6),
        output_cost_per_token: Some(2e-6),
        ..Default::default()
    };

    let pricing = pricing.with_user_overrides([(&key, &rates)]);
    let entry = pricing.find(&key).unwrap();
    assert_eq!((entry.input, entry.output), (1e-6, 2e-6));
    assert!(pricing.rows.folded_indexes.get().is_none());
    assert_eq!(pricing.rows.decode_cache_counts(), (0, 0));
}

#[test]
fn embedded_table_matches_the_rows_loaded_from_its_snapshots() {
    let [litellm, models_dev] = eager_catalogues();
    let embedded = PricingMap::load_embedded();

    assert_eq!(sorted_rows(&embedded.rows), sorted_rows(&litellm.rows));
    assert_eq!(
        sorted_rows(&embedded_models_dev_pricing().rows),
        sorted_rows(&models_dev.rows)
    );
    assert_lookup_parity(&embedded, &litellm);
    assert_lookup_parity(embedded_models_dev_pricing(), &models_dev);
}

fn eager_catalogues() -> [PricingMap; 2] {
    crate::test_support::install_pricing_snapshots();
    let mut models_dev = PricingMap::default();
    models_dev
        .load_models_dev_json_missing(build_time_models_dev_json())
        .unwrap();
    let mut litellm = PricingMap::default();
    litellm.load_json(build_time_pricing_json());
    litellm.complete_layers(&models_dev);
    [litellm, models_dev]
}

fn assert_lookup_parity(embedded: &PricingMap, eager: &PricingMap) {
    let matched_key = |found: RowMatch<'_>| found.key.to_owned();
    for found in eager.rows.matches() {
        let key = found.key;
        let vendor = vendor_lookup_id(found).unwrap_or(key);
        for fold in [false, true] {
            let query = if fold {
                key.to_ascii_lowercase()
            } else {
                key.to_owned()
            };
            let cached = OnceLock::new();
            let name = Name::new(&query, &cached);
            let exact =
                |rows: &PricingRows| rows.exact(name, Lookup::Pricing, fold).map(matched_key);
            assert_eq!(
                exact(&embedded.rows),
                exact(&eager.rows),
                "exact {key} fold={fold}"
            );
            for (index, query, label) in [
                (Index::VendorId, vendor, "vendor"),
                (Index::BareHosted, key, "bare-hosted"),
            ] {
                let query = normalized_pricing_key(query);
                let query = if fold {
                    query.to_ascii_lowercase()
                } else {
                    query.into_owned()
                };
                let indexed = |rows: &PricingRows| {
                    let mut keys: Vec<_> = rows
                        .indexed_rows(&query, index, fold)
                        .map(matched_key)
                        .collect();
                    keys.sort_unstable();
                    keys
                };
                assert_eq!(
                    indexed(&embedded.rows),
                    indexed(&eager.rows),
                    "{label} {key} fold={fold}"
                );
            }
            for (host, _) in (0u8..).zip(data::HOSTS) {
                let host_row = |rows: &PricingRows| {
                    rows.host_row(name, host, Lookup::Pricing, fold)
                        .map(matched_key)
                };
                assert_eq!(
                    host_row(&embedded.rows),
                    host_row(&eager.rows),
                    "host {host} {key} fold={fold}"
                );
            }
        }
        let identity = normalized_pricing_key(key);
        let exact_only = |rows: &PricingRows| {
            (
                rows.contains_exact_only_identity(key, &identity),
                rows.exact_only_id(&identity).map(str::to_owned),
            )
        };
        assert_eq!(
            exact_only(&embedded.rows),
            exact_only(&eager.rows),
            "exact-only {key}"
        );

        let query = format!("parity-host/{identity}");
        let cached = OnceLock::new();
        let name = Name::new(&query, &cached);
        let fuzzy = |map: &PricingMap| {
            map.search_fuzzy(name, Lookup::Pricing, None, &|| None)
                .and_then(|hit| map.rows.at(hit.id()))
                .map(matched_key)
        };
        let expected = fuzzy(eager);
        assert_eq!(fuzzy(embedded), expected, "fuzzy {key}");
        assert!(
            expected.is_none() || embedded.rows.may_match_fuzzy(&query),
            "secondary index rejected a fuzzy hit for {key}"
        );
    }
}

#[test]
fn embedded_query_variants_match_eager_exact_and_fuzzy_lookups() {
    let [eager_primary, eager_secondary] = eager_catalogues();
    let lazy_primary = PricingMap::load_embedded();
    let lazy_secondary = embedded_models_dev_pricing();
    let empty_primary = PricingMap::default();
    let queries: FxHashSet<_> = lazy_primary
        .rows
        .keys()
        .chain(lazy_secondary.rows.keys())
        .flat_map(|(_, key)| {
            [
                key.to_owned(),
                key.to_ascii_uppercase(),
                key.replace('.', "-"),
                key.replace('-', "."),
                format!("{key}-20250101"),
                format!("{key}[1m]"),
                format!("{key}:free"),
                format!("{key}@latest"),
                format!("x/{key}"),
                format!("openrouter/{key}"),
                format!("bedrock/{key}-v1:0"),
            ]
            .into_iter()
            .chain(bare_suffixes(key).map(str::to_owned))
        })
        .collect();
    let queries: Vec<_> = queries.into_iter().collect();
    assert!(!queries.is_empty());
    let workers = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    for (eager, lazy, secondary) in [
        (&eager_primary, &lazy_primary, false),
        (&eager_secondary, lazy_secondary, true),
    ] {
        let eager_keys = FuzzyKeys::new(&eager.rows);
        let lazy_keys = FuzzyKeys::new(&lazy.rows);
        let check = |query: &str| {
            let cached = OnceLock::new();
            let name = Name::new(query, &cached);
            for probe in std::iter::once(name).chain(name.suffixes()) {
                for fold in [false, true] {
                    let exact = |rows: &PricingRows| {
                        rows.exact(probe, Lookup::Pricing, fold)
                            .map(|found| found.key.to_owned())
                    };
                    assert_eq!(
                        exact(&lazy.rows),
                        exact(&eager.rows),
                        "exact {:?} from {query:?} (secondary={secondary}, fold={fold})",
                        probe.raw()
                    );
                }
            }
            let fuzzy = |map: &PricingMap, keys: &FuzzyKeys<'_>| {
                let candidates = keys.candidates(name);
                let (primary, fallback) = if secondary {
                    (&empty_primary, Some(map.rows.as_ref()))
                } else {
                    (map, None)
                };
                primary
                    .search_fuzzy_with_keys(name, Lookup::Pricing, fallback, &|| fallback, |rows| {
                        let keys = if std::ptr::eq(rows, map.rows.as_ref()) {
                            candidates.as_slice()
                        } else {
                            &[]
                        };
                        keys.iter().copied()
                    })
                    .map(|hit| {
                        assert_eq!(hit.is_models_dev(), secondary);
                        map.rows.at(hit.id()).unwrap().key.to_owned()
                    })
            };
            assert_eq!(
                fuzzy(lazy, &lazy_keys),
                fuzzy(eager, &eager_keys),
                "fuzzy {query:?} (secondary={secondary})"
            );
        };
        thread::scope(|scope| {
            for chunk in queries.chunks(queries.len().div_ceil(workers)) {
                scope.spawn(|| chunk.iter().for_each(|query| check(query)));
            }
        });
    }
}

fn bare_suffixes(key: &str) -> impl Iterator<Item = &str> {
    boundaries(key)
        .filter(|&start| start > 0 && is_qualifier_end(key.as_bytes()[start - 1]))
        .map(|start| &key[start..])
}

fn boundaries(key: &str) -> impl Iterator<Item = usize> + '_ {
    key.char_indices()
        .map(|(index, _)| index)
        .chain([key.len()])
}

type FuzzyKeyIndex<'a> = FxHashMap<String, Vec<(usize, &'a str)>>;

struct FuzzyKeys<'a> {
    by_identity: FuzzyKeyIndex<'a>,
    by_suffix: FuzzyKeyIndex<'a>,
}

impl<'a> FuzzyKeys<'a> {
    fn new(rows: &'a PricingRows) -> Self {
        let mut by_identity = FuzzyKeyIndex::default();
        let mut by_suffix = FuzzyKeyIndex::default();
        for row @ (_, key) in rows.keys() {
            let identity = normalized_pricing_key(key);
            for start in boundaries(&identity).skip(1) {
                by_suffix
                    .entry(identity[start..].to_owned())
                    .or_default()
                    .push(row);
            }
            by_identity
                .entry(identity.into_owned())
                .or_default()
                .push(row);
        }
        Self {
            by_identity,
            by_suffix,
        }
    }

    // NOTE(2026-11-22, #2465): A fuzzy hit contains the candidate key or matches its suffix. Shortlisting both directions bounds test runtime without relying on the embedded filter.
    fn candidates(&self, name: Name<'_>) -> Vec<(usize, &'a str)> {
        let identity = name.identity();
        let mut rows = Vec::new();
        rows.extend(self.by_suffix.get(identity).into_iter().flatten().copied());
        for raw in std::iter::once(name.raw()).chain(bare_suffixes(name.raw())) {
            let suffix = &identity[identity.len() - raw.len()..];
            for end in boundaries(suffix) {
                rows.extend(
                    self.by_identity
                        .get(&suffix[..end])
                        .into_iter()
                        .flatten()
                        .copied(),
                );
            }
        }
        rows.sort_unstable_by_key(|(id, _)| *id);
        rows.dedup_by_key(|(id, _)| *id);
        rows
    }
}

#[test]
fn whole_request_catalogue_rows_bill_cache_at_the_long_context_rates() {
    use crate::pricing::LongContextMode;

    crate::test_support::install_pricing_snapshots();
    let primary = PricingMap::load_embedded();
    for catalogue in [&primary, embedded_models_dev_pricing()] {
        for found in catalogue.rows.matches() {
            let pricing = found.row.rates();
            let Some(tier) = pricing
                .long_context
                .filter(|tier| tier.mode == LongContextMode::WholeRequest)
            else {
                continue;
            };
            let model = found.key;
            let resolved = pricing.whole_request_rates(tier);
            let [read, write, write_1h] = expected_cache_multipliers(model);
            for (bucket, actual, published, base, multiplier) in [
                (
                    "read",
                    resolved.cache_read,
                    tier.rates.cache_read,
                    pricing.cache_read_rate_at(pricing.input),
                    read,
                ),
                (
                    "five-minute write",
                    resolved.cache_create,
                    tier.rates.cache_create,
                    pricing.cache_creation_rate(),
                    write,
                ),
                (
                    "one-hour write",
                    resolved.cache_create_1h,
                    tier.rates.cache_create_1h,
                    pricing.one_hour_cache_creation_rate(),
                    write_1h,
                ),
            ] {
                let Some(input) = tier.rates.input.filter(|_| published.is_none()) else {
                    assert_eq!(actual, published, "{model} {bucket}");
                    continue;
                };
                let actual = actual.unwrap_or_else(|| panic!("{model} {bucket} has no rate"));
                assert!(actual >= base, "{model} {bucket}: {actual} < {base}");
                if let Some(multiplier) = multiplier {
                    let expected = (input * multiplier).max(base);
                    assert!(
                        (actual - expected).abs() < 1e-15,
                        "{model} {bucket}: {actual} != {expected}"
                    );
                }
            }
        }
    }
}

fn expected_cache_multipliers(model: &str) -> [Option<f64>; 3] {
    let model = model.rsplit('/').next().unwrap().to_ascii_lowercase();
    let model = model
        .find(".claude-")
        .map_or(model.as_str(), |start| &model[start + 1..])
        .replace('.', "-");
    let model = model.strip_prefix("openai-").unwrap_or(&model);
    match model {
        model if model.starts_with("gemini-4") => [Some(0.05), None, None],
        model if model.starts_with("gemini-") => [Some(0.1), None, None],
        model if model.starts_with("grok-4-5") => [Some(0.15), None, None],
        model if model.starts_with("grok-") => [Some(0.25), None, None],
        model
            if [
                "gpt-6-1-sol",
                "gpt-6-1-terra",
                "gpt-6-2-sol",
                "gpt-6-2-astra",
            ]
            .iter()
            .any(|prefix| model.starts_with(prefix)) =>
        {
            [Some(0.05), Some(1.25), None]
        }
        model
            if ["gpt-5-6", "gpt-5-7", "gpt-5-8", "gpt-5-9", "gpt-6"]
                .iter()
                .any(|prefix| model.starts_with(prefix)) =>
        {
            [Some(0.1), Some(1.25), None]
        }
        model if model.starts_with("gpt-") => [Some(0.1), Some(1.0), None],
        model if model.starts_with("claude-") => match model {
            model
                if model.starts_with("claude-sonnet-5-5")
                    || model.starts_with("claude-opus-5-5") =>
            {
                [Some(0.05), Some(1.25), Some(2.0)]
            }
            model
                if [
                    "claude-fable-5-1",
                    "claude-fable-5-2",
                    "claude-mythos-5-1",
                    "claude-mythos-5-2",
                ]
                .iter()
                .any(|prefix| model.starts_with(prefix)) =>
            {
                [Some(0.025), Some(1.25), Some(2.0)]
            }
            _ => [Some(0.1), Some(1.25), Some(2.0)],
        },
        _ => [None; 3],
    }
}

#[test]
fn loads_embedded_claude_pricing() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    assert!(pricing.len() > 0);
    assert!(pricing.find("claude-sonnet-4-20250514").is_some());
}

#[test]
fn a_models_dev_load_preserves_an_existing_embedded_row() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::load_embedded();
    let before = pricing.find("gpt-5.5").unwrap();
    assert_eq!(
        pricing.load_models_dev_json_missing(r#"{"gpt-5.5":{"cost":{"input":100,"output":200}}}"#),
        Some(0),
    );
    assert_eq!(pricing.find("gpt-5.5"), Some(before));
}

#[test]
fn embedded_pricing_includes_hermes_frontier_models() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    assert!(pricing.find("gpt-5.5").is_some());
    assert!(pricing.find("grok-4.3").is_some());
    assert_eq!(pricing.context_limit("grok-4.3"), Some(1_000_000));
}

#[test]
fn embedded_pricing_includes_moonshot_kimi_for_offline_reports() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let kimi_k25 = pricing.find("moonshot/kimi-k2.5").unwrap();
    let kimi_k26 = pricing.find("moonshot/kimi-k2.6").unwrap();

    assert_eq!(kimi_k25.input, 0.6e-6);
    assert_eq!(kimi_k25.output, 3e-6);
    assert_eq!(kimi_k25.cache_read, Some(0.1e-6));
    assert_eq!(kimi_k26.input, 0.95e-6);
    assert_eq!(kimi_k26.output, 4e-6);
    assert_eq!(kimi_k26.cache_read, Some(0.16e-6));
    assert_eq!(pricing.context_limit("moonshot/kimi-k2.5"), Some(262_144));
    assert_eq!(pricing.context_limit("moonshot/kimi-k2.6"), Some(262_144));
}

#[test]
fn offline_prices_models_outside_the_claude_and_moonshot_families() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let grok_build = pricing
        .find("grok-build-0.1")
        .expect("embedded models.dev should include xAI pricing");

    assert!((grok_build.input * 1e6 - 1.0).abs() < 1e-9);
    assert!((grok_build.output * 1e6 - 2.0).abs() < 1e-9);
    assert_eq!(pricing.context_limit("grok-build-0.1"), Some(256_000));
}

#[test]
fn offline_prices_grok_4_7_fast_at_xai_list_rates_with_its_long_context_tier() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let fast = pricing.find("grok-4.7-fast").unwrap();

    assert_eq!(
        (fast.input, fast.output, fast.cache_create, fast.cache_read),
        (4e-6, 12e-6, Some(4e-6), Some(1e-6))
    );
    let long = fast.long_context.unwrap();
    assert_eq!(long.threshold, 199_999);
    assert_eq!(long.mode, super::super::LongContextMode::WholeRequest);
    assert_eq!(
        (
            long.rates.input,
            long.rates.output,
            long.rates.cache_create,
            long.rates.cache_read
        ),
        (Some(6e-6), Some(18e-6), Some(6e-6), Some(1.5e-6))
    );
    assert_eq!(pricing.context_limit("grok-4.7-fast"), Some(500_000));
}

#[test]
fn embedded_pricing_includes_z_ai_glm_models_for_offline_reports() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    let glm_51 = pricing.find("glm-5.1").unwrap();
    assert_eq!(glm_51.input, 1.4e-6);
    assert_eq!(glm_51.output, 4.4e-6);
    assert_eq!(glm_51.cache_create, Some(0.0));
    assert_eq!(glm_51.cache_read, Some(0.26e-6));

    let glm_5 = pricing.find("glm-5").unwrap();
    assert_eq!(glm_5.input, 1.0e-6);
    assert_eq!(glm_5.output, 3.2e-6);
    assert_eq!(glm_5.cache_create, Some(0.0));
    assert_eq!(glm_5.cache_read, Some(0.2e-6));
    assert_eq!(pricing.context_limit("zai/glm-5"), Some(200_000));

    let glm_5_turbo = pricing.find("glm-5-turbo").unwrap();
    assert_eq!(glm_5_turbo.input, 1.2e-6);
    assert_eq!(glm_5_turbo.output, 4.0e-6);
    assert_eq!(glm_5_turbo.cache_create, Some(0.0));
    assert_eq!(glm_5_turbo.cache_read, Some(0.24e-6));

    let glm_47 = pricing.find("glm-4.7").unwrap();
    assert_eq!(glm_47.input, 0.6e-6);
    assert_eq!(glm_47.output, 2.2e-6);
    assert_eq!(glm_47.cache_create, Some(0.0));
    assert_eq!(glm_47.cache_read, Some(0.11e-6));

    let glm_46 = pricing.find("glm-4.6").unwrap();
    assert_eq!(glm_46.input, 0.6e-6);
    assert_eq!(glm_46.output, 2.2e-6);
    assert_eq!(glm_46.cache_create, Some(0.0));
    assert_eq!(glm_46.cache_read, Some(0.11e-6));

    let glm_45 = pricing.find("glm-4.5").unwrap();
    assert_eq!(glm_45.input, 0.6e-6);
    assert_eq!(glm_45.output, 2.2e-6);
    assert_eq!(glm_45.cache_create, Some(0.0));
    assert_eq!(glm_45.cache_read, Some(0.11e-6));

    let zai_glm_45 = pricing.find("zai/glm-4.5").unwrap();
    assert_eq!(zai_glm_45.input, 0.6e-6);
    assert_eq!(zai_glm_45.output, 2.2e-6);
    assert_eq!(zai_glm_45.cache_create, Some(0.0));
    assert_eq!(zai_glm_45.cache_read, Some(0.11e-6));
    assert_eq!(pricing.context_limit("zai/glm-4.5"), Some(128_000));
}

#[test]
fn embedded_pricing_resolves_overlapping_model_keys_exactly() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let sonnet_4 = pricing.find("claude-sonnet-4-20250514").unwrap();
    let sonnet_45 = pricing.find("claude-sonnet-4-5-20250929").unwrap();

    assert_eq!(
        pricing.find("claude-sonnet-4-20250514").unwrap().input,
        sonnet_4.input
    );
    assert_eq!(
        pricing.find("claude-sonnet-4-5-20250929").unwrap().input,
        sonnet_45.input,
    );
    assert_eq!(
        pricing
            .find("anthropic.claude-sonnet-4-20250514-v1:0")
            .unwrap()
            .input,
        sonnet_4.input,
    );
    assert_eq!(
        pricing.find("claude-3-5-haiku-20241022").unwrap().input,
        0.8e-6,
    );
}

#[test]
fn embedded_pricing_includes_gpt_5_5_for_offline_codex_reports() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let gpt_55 = pricing.find("gpt-5.5").unwrap();

    assert_eq!(gpt_55.input, 5e-6);
    assert_eq!(gpt_55.output, 30e-6);
    assert_eq!(gpt_55.cache_read, Some(0.5e-6));
    assert_eq!(gpt_55.fast_multiplier, 2.5);
    assert_eq!(pricing.context_limit("gpt-5.5"), Some(1_050_000));
}

#[test]
fn embedded_pricing_preserves_gpt_5_6_generated_sources() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let mut litellm_snapshot = PricingMap::default();
    litellm_snapshot.load_json(build_time_pricing_json());
    let models_dev_snapshot = embedded_models_dev_pricing();

    for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        let entry = pricing.find(model).unwrap();
        let litellm_source = litellm_snapshot
            .resolve_at(None, &[(model, MatchKind::Exact)], None)
            .map(|(_, rates)| rates);
        let base_source = litellm_source
            .or_else(|| {
                models_dev_snapshot
                    .resolve_at(None, &[(model, MatchKind::Exact)], None)
                    .map(|(_, rates)| rates)
            })
            .unwrap();
        let tier_source = litellm_source
            .filter(|source| {
                source
                    .long_context
                    .and_then(|context| context.rates.input)
                    .is_some()
                    || source
                        .long_context
                        .and_then(|context| context.rates.output)
                        .is_some()
                    || source
                        .long_context
                        .and_then(|context| context.rates.cache_create)
                        .is_some()
                    || source
                        .long_context
                        .and_then(|context| context.rates.cache_read)
                        .is_some()
            })
            .or_else(|| {
                models_dev_snapshot
                    .resolve_at(None, &[(model, MatchKind::Exact)], None)
                    .map(|(_, rates)| rates)
            })
            .unwrap();
        assert_eq!(entry.input, base_source.input, "{model}");
        assert_eq!(entry.output, base_source.output, "{model}");
        assert_eq!(entry.cache_create, base_source.cache_create, "{model}");
        assert_eq!(entry.cache_read, base_source.cache_read, "{model}");
        assert_eq!(
            entry.long_context.and_then(|context| context.rates.input),
            tier_source
                .long_context
                .and_then(|context| context.rates.input),
            "{model}"
        );
        assert_eq!(
            entry.long_context.and_then(|context| context.rates.output),
            tier_source
                .long_context
                .and_then(|context| context.rates.output),
            "{model}"
        );
        assert_eq!(
            entry
                .long_context
                .and_then(|context| context.rates.cache_create),
            tier_source
                .long_context
                .and_then(|context| context.rates.cache_create),
            "{model}"
        );
        assert_eq!(
            entry
                .long_context
                .and_then(|context| context.rates.cache_read),
            tier_source
                .long_context
                .and_then(|context| context.rates.cache_read),
            "{model}"
        );
        assert_eq!(
            entry.long_context.map(|context| context.threshold),
            tier_source.long_context.map(|context| context.threshold),
            "{model}"
        );
        assert_eq!(
            pricing.context_limit(model),
            litellm_snapshot
                .rows
                .get(model)
                .and_then(|row| row.context_limit.as_ref())
                .copied()
                .or_else(|| models_dev_snapshot
                    .rows
                    .get(model)
                    .and_then(|row| row.context_limit.as_ref())
                    .copied())
                .or_else(|| super::super::BUILTIN_PRICING
                    .iter()
                    .find(|entry| entry.model == model)
                    .and_then(|entry| entry.context_limit)),
            "{model}"
        );
    }
}

#[test]
fn builtin_gpt_5_6_fallbacks_preserve_source_precedence() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.6-sol": {
                    "input_cost_per_token": 0.000004123,
                    "output_cost_per_token": 0.000020123,
                    "cache_creation_input_token_cost": 0.000005123,
                    "cache_read_input_token_cost": 0.0000004123,
                    "max_input_tokens": 654321
                }
            }"#,
    );
    let loaded = pricing
        .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    let loaded_context_limit = pricing.context_limit("gpt-5.6-sol");

    let mut builtin = PricingMap::default();
    builtin.put_builtin_pricing();
    let builtin_rates = builtin
        .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_ne!(loaded.input, builtin_rates.input);
    assert_ne!(loaded.output, builtin_rates.output);

    pricing.put_builtin_pricing();
    let resolved = pricing
        .resolve_at(None, &[("gpt-5.6-sol", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    assert_eq!(resolved.input, loaded.input);
    assert_eq!(resolved.output, loaded.output);
    assert_eq!(resolved.cache_create, loaded.cache_create);
    assert_eq!(resolved.cache_read, loaded.cache_read);
    assert_eq!(resolved.fast_multiplier, loaded.fast_multiplier);
    assert_eq!(pricing.context_limit("gpt-5.6-sol"), loaded_context_limit);

    let mut embedded_models_dev = PricingMap::default();
    embedded_models_dev.insert_test_limit("gpt-5.6-sol", 777_777);
    let mut live_models_dev = PricingMap::default();
    live_models_dev.insert_test_limit("gpt-5.6-sol", 888_888);
    assert_eq!(
        layered(&pricing, Some(&embedded_models_dev))
            .context_limit_with_live("gpt-5.6-sol", || Some(&live_models_dev)),
        Some(654_321)
    );
    assert_eq!(
        layered(&PricingMap::default(), Some(&embedded_models_dev))
            .context_limit_with_live("gpt-5.6-sol", || Some(&live_models_dev)),
        Some(1_050_000)
    );
}

#[test]
fn gpt_5_6_alias_resolves_to_sol_when_the_generic_entry_is_missing() {
    crate::test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.6-sol": {
                    "input_cost_per_token": 0.000004,
                    "output_cost_per_token": 0.00002,
                    "cache_creation_input_token_cost": 0.000005,
                    "cache_read_input_token_cost": 0.0000004,
                    "input_cost_per_token_above_200k_tokens": 0.000008,
                    "output_cost_per_token_above_200k_tokens": 0.00003,
                    "cache_creation_input_token_cost_above_200k_tokens": 0.00001,
                    "cache_read_input_token_cost_above_200k_tokens": 0.0000008,
                    "max_input_tokens": 1050000
                }
            }"#,
    );
    let alias = pricing.find("gpt-5.6").unwrap();
    let sol = pricing.find("gpt-5.6-sol").unwrap();

    assert_eq!(alias.input, sol.input);
    assert_eq!(alias.output, sol.output);
    assert_eq!(alias.cache_create, sol.cache_create);
    assert_eq!(alias.cache_read, sol.cache_read);
    assert_eq!(
        alias.long_context.and_then(|context| context.rates.input),
        sol.long_context.and_then(|context| context.rates.input)
    );
    assert_eq!(
        alias.long_context.and_then(|context| context.rates.output),
        sol.long_context.and_then(|context| context.rates.output)
    );
    assert_eq!(
        pricing.context_limit("gpt-5.6"),
        pricing.context_limit("gpt-5.6-sol")
    );
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-5.6")
            .unwrap()
            .long_context_split_threshold(),
        PricingMap::load_embedded()
            .find("gpt-5.6-sol")
            .unwrap()
            .long_context_split_threshold()
    );
}

#[test]
fn embedded_gpt_5_6_exact_entry_wins_over_the_sol_alias() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let exact = pricing
        .resolve_at(None, &[("gpt-5.6", MatchKind::Exact)], None)
        .map(|(_, rates)| rates)
        .unwrap();
    let resolved = pricing.find("gpt-5.6").unwrap();

    assert_eq!(resolved.input, exact.input);
    assert_eq!(resolved.output, exact.output);
    assert_eq!(resolved.cache_create, exact.cache_create);
    assert_eq!(resolved.cache_read, exact.cache_read);
    assert_eq!(
        resolved
            .long_context
            .and_then(|context| context.rates.input),
        exact.long_context.and_then(|context| context.rates.input)
    );
    assert_eq!(
        resolved
            .long_context
            .and_then(|context| context.rates.output),
        exact.long_context.and_then(|context| context.rates.output)
    );
    assert_eq!(
        resolved.long_context.map(|context| context.threshold),
        exact.long_context.map(|context| context.threshold)
    );
    assert_eq!(
        pricing.context_limit("gpt-5.6"),
        pricing
            .rows
            .get("gpt-5.6")
            .and_then(|row| row.context_limit.as_ref())
            .copied()
    );
}

#[test]
fn embedded_pricing_does_not_resolve_undated_codex_auto_review_model() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();

    assert!(pricing.find("codex-auto-review").is_none());
    assert!(pricing.context_limit("codex-auto-review").is_none());
}

#[test]
fn embedded_pricing_resolves_codex_spark_short_model_alias() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let short_spark = pricing
        .find("gpt-5.3-spark")
        .expect("gpt-5.3-spark should resolve via model alias");
    let codex_spark = pricing
        .find("gpt-5.3-codex-spark")
        .expect("canonical Codex Spark pricing should exist");

    assert_eq!(short_spark.input, codex_spark.input);
    assert_eq!(short_spark.output, codex_spark.output);
    assert_eq!(short_spark.cache_read, codex_spark.cache_read);
    assert_eq!(short_spark.fast_multiplier, codex_spark.fast_multiplier);
}

#[test]
fn embedded_pricing_resolves_codex_gpt_reserve_to_gpt_5_6_luna() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let reserve = pricing
        .find("gpt-reserve")
        .expect("gpt-reserve should resolve via model alias");
    let luna = pricing
        .find("gpt-5.6-luna")
        .expect("GPT-5.6 Luna pricing should exist");

    assert_eq!(reserve.input, luna.input);
    assert_eq!(reserve.output, luna.output);
    assert_eq!(reserve.cache_read, luna.cache_read);
    assert_eq!(reserve.fast_multiplier, luna.fast_multiplier);
    assert_eq!(
        PricingMap::load_embedded()
            .find("gpt-reserve")
            .unwrap()
            .long_context_split_threshold(),
        PricingMap::load_embedded()
            .find("gpt-5.6-luna")
            .unwrap()
            .long_context_split_threshold()
    );
}

#[test]
fn embedded_pricing_resolves_opus_47_dot_model_names() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let opus_47 = pricing.find("claude-opus-4-7").unwrap();

    assert_eq!(
        pricing.find("claude-opus-4.7-20260416").unwrap().input,
        opus_47.input
    );
    assert_eq!(
        pricing.context_limit("claude-opus-4.7"),
        pricing.context_limit("claude-opus-4-7")
    );
}

#[test]
fn embedded_pricing_resolves_opus_48_dot_model_names() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let canonical = pricing.find("claude-opus-4-8").unwrap();

    let opus_48 = pricing.find("claude-opus-4.8-20260528").unwrap();
    assert_eq!(opus_48.input, canonical.input);
    assert_eq!(opus_48.output, canonical.output);
    assert_eq!(opus_48.cache_create, canonical.cache_create);
    assert_eq!(opus_48.cache_read, canonical.cache_read);
    assert_eq!(
        pricing.context_limit("claude-opus-4.8"),
        pricing.context_limit("claude-opus-4-8")
    );
}

#[test]
fn embedded_pricing_resolves_separator_aliases_for_other_claude_models() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let sonnet_46 = pricing.find("claude-sonnet-4-6").unwrap();
    let haiku_45 = pricing.find("claude-haiku-4-5").unwrap();

    assert_eq!(
        pricing.find("claude-sonnet-4.6-20260416").unwrap().input,
        sonnet_46.input
    );
    assert_eq!(
        pricing.find("claude-haiku-4.5").unwrap().input,
        haiku_45.input
    );
    assert_eq!(
        pricing.context_limit("claude-sonnet-4.6"),
        pricing.context_limit("claude-sonnet-4-6")
    );
    assert_eq!(
        pricing.context_limit("claude-haiku-4.5"),
        pricing.context_limit("claude-haiku-4-5")
    );
}

#[test]
fn offline_runs_bill_xai_ids_from_xais_own_rows_like_online_runs() {
    crate::test_support::install_pricing_snapshots();
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(build_time_pricing_json()));
        Ok(())
    });
    for (model, [input, output, read, long_input]) in [
        ("grok-code-fast-1", [1e-6, 2e-6, 0.2e-6, 2e-6]),
        ("grok-4.20", [1.25e-6, 2.5e-6, 0.2e-6, 2.5e-6]),
        (
            "grok-4.20-0309-reasoning",
            [1.25e-6, 2.5e-6, 0.2e-6, 2.5e-6],
        ),
        ("grok-4.7", [2e-6, 6e-6, 0.5e-6, 4e-6]),
    ] {
        for (mode, pricing) in [("offline", &offline), ("online", &online)] {
            let (_, rates) = pricing
                .resolve_at(Some(model), &[(model, MatchKind::Fuzzy)], None)
                .unwrap();
            assert_eq!(
                [
                    rates.input,
                    rates.output,
                    rates.cache_read_rate_at(rates.input),
                    rates
                        .long_context
                        .and_then(|context| context.rates.input)
                        .unwrap_or(rates.input),
                ],
                [input, output, read, long_input],
                "{mode} {model}"
            );
            assert_eq!(
                rates.long_context.map(|context| context.mode),
                Some(super::super::LongContextMode::WholeRequest),
                "{mode} {model}"
            );
        }
    }
}

fn xai_cost(
    pricing: &PricingMap,
    model: &str,
    input_tokens: u64,
    timestamp: Option<crate::TimestampMs>,
) -> f64 {
    let candidates = [(model, MatchKind::Fuzzy)];
    crate::cost::price_entry(
        crate::cost::EntryPricing {
            granularity: crate::cost::UsageGranularity::Request,
            usage: crate::TokenUsageRaw {
                input_tokens,
                output_tokens: 1_000,
                ..Default::default()
            },
            reasoning_tokens: 0,
            reported_model: Some(model),
            candidates: &candidates,
            stored_cost: None,
            timestamp,
        },
        crate::cost::PriceTable::Calculate(pricing),
    )
    .cost
}

#[test]
fn xai_bills_the_long_context_rate_from_a_200k_token_prompt() {
    crate::test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for (input_tokens, expected) in [
        (199_999, 199_999.0 * 1.25e-6 + 1_000.0 * 2.5e-6),
        (200_000, 200_000.0 * 2.5e-6 + 1_000.0 * 5e-6),
    ] {
        let cost = xai_cost(&pricing, "grok-4.20", input_tokens, None);
        assert!(
            (cost - expected).abs() < 1e-9,
            "{input_tokens}: {cost} != {expected}"
        );
    }
}

#[test]
fn grok_code_fast_1_bills_its_own_rates_until_xai_aliased_it_to_grok_build() {
    crate::test_support::install_pricing_snapshots();
    let offline = PricingMap::load_embedded();
    let mut online = PricingMap::load_embedded();
    online.refresh_litellm(|load| {
        assert!(load(build_time_pricing_json()));
        Ok(())
    });
    let before = super::timestamp("2026-05-15T14:59:11Z");
    let after = super::timestamp("2026-05-15T14:59:12Z");
    for (mode, pricing) in [("offline", &offline), ("online", &online)] {
        for model in [
            "grok-code-fast-1",
            "grok-code-fast",
            "grok-code-fast-1-0825",
        ] {
            for (instant, input_tokens, expected) in [
                (before, 1_000, 1_000.0 * 0.2e-6 + 1_000.0 * 1.5e-6),
                (before, 250_000, 250_000.0 * 0.2e-6 + 1_000.0 * 1.5e-6),
                (after, 1_000, 1_000.0 * 1e-6 + 1_000.0 * 2e-6),
                (after, 250_000, 250_000.0 * 2e-6 + 1_000.0 * 4e-6),
            ] {
                let cost = xai_cost(pricing, model, input_tokens, Some(instant));
                assert!(
                    (cost - expected).abs() < 1e-9,
                    "{mode} {model} {input_tokens}: {cost} != {expected}"
                );
            }
        }
    }
}
