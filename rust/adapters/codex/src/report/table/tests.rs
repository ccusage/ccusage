use super::*;
use crate::{CodexModelUsage, CodexServiceTier, report::json::totals_json};

#[test]
fn report_totals_saturate_across_periods() {
    let group = CodexGroup {
        input_tokens: u64::MAX,
        output_tokens: u64::MAX,
        reasoning_output_tokens: u64::MAX,
        total_tokens: u64::MAX,
        ..CodexGroup::default()
    };
    let totals = totals_json(
        [&group, &group].into_iter(),
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
    );
    for field in [
        "inputTokens",
        "outputTokens",
        "reasoningOutputTokens",
        "totalTokens",
    ] {
        assert_eq!(totals[field], u64::MAX);
    }
    let mut table = CodexTableTotals::default();
    table.add(&group, u64::MAX, 0.0);
    table.add(&group, u64::MAX, 0.0);
    assert_eq!(table.input_tokens, u64::MAX);
    assert_eq!(table.output_tokens, u64::MAX);
    assert_eq!(table.reasoning_output_tokens, u64::MAX);
    assert_eq!(table.total_tokens, u64::MAX);
}

#[test]
fn table_rows_and_totals_render_cache_creation_tokens() {
    let group = CodexGroup {
        input_tokens: 100,
        cached_input_tokens: 60,
        cache_creation_tokens: 30,
        output_tokens: 5,
        reasoning_output_tokens: 1,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    let shared = SharedArgs {
        color: ccusage_core::cli::ColorChoice::Never,
        ..SharedArgs::default()
    };
    let (headers, columns) = codex_table_columns("Date", false);
    let (row, input_tokens, cost) = codex_table_row(
        "2026-08-20",
        &group,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        false,
    );
    let mut totals = CodexTableTotals::default();
    totals.add(&group, input_tokens, cost);
    let total_row = codex_table_total_row(&totals, &shared, false);

    assert_eq!(headers[5], "Cache Create");
    assert_eq!(headers.len(), columns.len());
    assert_eq!(row[5], "30");
    assert_eq!(total_row[5], "30");
}

#[test]
fn daily_rows_leave_date_compaction_to_the_table() {
    let group = CodexGroup {
        input_tokens: 100,
        cached_input_tokens: 60,
        cache_creation_tokens: 30,
        output_tokens: 5,
        reasoning_output_tokens: 1,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    let (headers, columns) = codex_table_columns("Date", false);
    let (row, _, _) = codex_table_row(
        "2026-08-20",
        &group,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        false,
    );
    let (no_cost_headers, no_cost_columns) = codex_table_columns("Date", true);
    let (no_cost_row, _, _) = codex_table_row(
        "2026-08-20",
        &group,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        true,
    );

    assert_eq!(headers[5], "Cache Create");
    assert_eq!(headers.len(), columns.len());
    assert_eq!(row[0], "2026-08-20");
    assert_eq!(no_cost_headers.last(), Some(&"Total Tokens"));
    assert_eq!(no_cost_headers.len(), no_cost_columns.len());
    assert_eq!(no_cost_row[0], "2026-08-20");
}

#[test]
fn no_cost_daily_columns_match_detail_and_total_rows() {
    let group = CodexGroup {
        input_tokens: 100,
        cached_input_tokens: 60,
        cache_creation_tokens: 30,
        output_tokens: 5,
        reasoning_output_tokens: 1,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    let shared = SharedArgs {
        color: ccusage_core::cli::ColorChoice::Never,
        ..SharedArgs::default()
    };
    let (headers, columns) = codex_table_columns("Date", true);
    let (first_row, first_input, first_cost) = codex_table_row(
        "2026-08-20",
        &group,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        true,
    );
    let (second_row, second_input, second_cost) = codex_table_row(
        "2026-08-21",
        &group,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        true,
    );
    let mut totals = CodexTableTotals::default();
    totals.add(&group, first_input, first_cost);
    totals.add(&group, second_input, second_cost);
    let total_row = codex_table_total_row(&totals, &shared, true);

    assert_eq!(headers[5], "Cache Create");
    assert_eq!(headers.last(), Some(&"Total Tokens"));
    assert_eq!(headers.len(), columns.len());
    assert_eq!(first_row.len(), headers.len());
    assert_eq!(second_row.len(), headers.len());
    assert_eq!(total_row.len(), headers.len());
    assert_eq!(first_row[0], "2026-08-20");
    assert_eq!(second_row[0], "2026-08-21");
    assert_ne!(first_row[0], second_row[0]);
    assert_eq!(first_row[5], "30");
    assert_eq!(total_row[5], "60");
}

fn breakdown_group() -> CodexGroup {
    let mut group = CodexGroup {
        input_tokens: 100,
        cached_input_tokens: 60,
        cache_creation_tokens: 30,
        output_tokens: 5,
        reasoning_output_tokens: 1,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    group.models.insert(
        "gpt-5.3-codex".to_string(),
        CodexModelUsage {
            reasoning_output_tokens: 1,
            total_tokens: 73,
            ..CodexModelUsage::from_buckets(
                crate::CodexUsageBucket {
                    input_tokens: 70,
                    cached_input_tokens: 40,
                    cache_creation_tokens: 20,
                    output_tokens: 3,
                    ..Default::default()
                },
                Default::default(),
            )
        },
    );
    group.models.insert(
        "gpt-5-mini".to_string(),
        CodexModelUsage {
            reasoning_output_tokens: 0,
            total_tokens: 32,
            ..CodexModelUsage::from_buckets(
                crate::CodexUsageBucket {
                    input_tokens: 30,
                    cached_input_tokens: 20,
                    cache_creation_tokens: 10,
                    output_tokens: 2,
                    ..Default::default()
                },
                Default::default(),
            )
        },
    );
    group
}

#[test]
fn breakdown_rows_render_one_sub_row_per_model_with_matching_totals() {
    let group = breakdown_group();
    let shared = SharedArgs {
        breakdown: true,
        color: ccusage_core::cli::ColorChoice::Never,
        ..SharedArgs::default()
    };
    let rows = enabled_codex_breakdown_rows(
        &group,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        &shared,
    );

    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row[0].contains("└─")));
    assert!(rows[0][0].contains("gpt-5.3-codex") || rows[1][0].contains("gpt-5.3-codex"));
    assert!(rows[0][0].contains("gpt-5-mini") || rows[1][0].contains("gpt-5-mini"));
    for (model, usage) in &group.models {
        let rendered = rows.iter().find(|row| row[0].contains(model)).unwrap();
        assert_eq!(rendered[7], format_number(usage.total_tokens));
    }

    let input: u64 = group
        .models
        .values()
        .map(|usage| {
            non_cached_input_tokens(
                usage.usage.input_tokens,
                usage.usage.cached_input_tokens,
                usage.usage.cache_creation_tokens,
            )
        })
        .sum();
    let (row, group_input, _) = codex_table_row(
        "2026-08-20",
        &group,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        false,
    );
    assert_eq!(input, group_input);
    assert_eq!(row[2], format_number(group_input));
    let total: u64 = group.models.values().map(|usage| usage.total_tokens).sum();
    assert_eq!(total, group.total_tokens);
}

#[test]
fn table_pushes_no_extra_rows_without_breakdown_flag() {
    let group = breakdown_group();
    let shared = SharedArgs {
        color: ccusage_core::cli::ColorChoice::Never,
        ..SharedArgs::default()
    };
    let (headers, columns) = codex_table_columns("Date", false);
    let (row, _, _) = codex_table_row(
        "2026-08-20",
        &group,
        Some(&PricingMap::default()),
        CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
        false,
    );

    assert!(!shared.breakdown);
    assert!(
        enabled_codex_breakdown_rows(
            &group,
            Some(&PricingMap::default()),
            CodexSpeedPolicy::Forced(CodexServiceTier::Standard),
            &shared,
        )
        .is_empty()
    );
    assert_eq!(row.len(), headers.len());
    assert_eq!(headers.len(), columns.len());
}
