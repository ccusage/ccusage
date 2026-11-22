use super::{wrap::compact_date_cell, *};

#[test]
fn compact_date_cell_splits_iso_dates() {
    assert_eq!(
        compact_date_cell("2026-05-18"),
        Some("2026\n05-18".to_string())
    );
    assert_eq!(compact_date_cell("20260518"), None);
}

#[test]
fn width_fitting_keeps_table_within_terminal_when_possible() {
    let widths = fit_widths_to_terminal(
        vec![20, 40, 14, 14],
        &[
            ColumnKind::Primary,
            ColumnKind::Detail,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        60,
        12,
        false,
        ContentWidths {
            numeric: &[],
            text: &[],
        },
        false,
    );

    assert!(cli_table_required_width(&widths) <= 60);
}

#[test]
fn numeric_columns_keep_the_reverted_minimum_when_space_is_tight() {
    let widths = fit_widths_to_terminal(
        vec![20, 40, 14, 14],
        &[
            ColumnKind::Primary,
            ColumnKind::Detail,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        49,
        12,
        false,
        ContentWidths {
            numeric: &[],
            text: &[],
        },
        false,
    );

    assert_eq!(widths[2], 10);
    assert_eq!(widths[3], 10);
}

#[test]
fn snapshots_full_table_with_multiline_cells_and_separators() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input", "Output", "Cost (USD)"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120);
    table.push(vec![
        "2026-05-18".to_string(),
        "- claude-sonnet-4\n- gpt-5.2-codex".to_string(),
        "1,234".to_string(),
        "56".to_string(),
        "$0.42".to_string(),
    ]);
    table.push_aligned(
        vec![
            "(assuming cache warmup)".to_string(),
            String::new(),
            "0".to_string(),
            "0".to_string(),
            "$0.00".to_string(),
        ],
        Align::Right,
    );
    table.separator();
    table.push(vec![
        "Total".to_string(),
        String::new(),
        "1,234".to_string(),
        "56".to_string(),
        "$0.42".to_string(),
    ]);

    insta::assert_snapshot!(table.render_lines().join("\n"));
}

#[test]
fn right_aligns_every_line_of_a_wrapped_label() {
    let mut table = SimpleTable::new(
        vec!["Label"],
        vec![ColumnKind::Primary],
        TerminalStyle::default(),
    )
    .with_terminal_width(12);
    table.push_aligned(vec!["longer small tiny".to_string()], Align::Right);

    let rendered = table.render_lines();
    assert_eq!(
        &rendered[3..rendered.len() - 1],
        ["│   longer │", "│    small │", "│     tiny │"]
    );
}

#[test]
fn snapshots_narrow_table_with_wrapping_truncation_and_compact_dates() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input", "Output", "Cost (USD)"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(56)
    .with_date_compaction(true);
    table.push(vec![
        "2026-05-18".to_string(),
        "- claude-sonnet-4-20250514\n- unusually-long-model-name-without-breaks".to_string(),
        "123,456,789".to_string(),
        "9,876,543".to_string(),
        "$12345.67".to_string(),
    ]);

    insta::assert_snapshot!(table.render_lines().join("\n"));
}

#[test]
fn column_widths_uses_max_line_not_sum_for_multiline_cells() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input", "Output", "Cost (USD)"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(200);
    table.push(vec![
        "2026-05-18".to_string(),
        "- claude-sonnet-4-20250514 (self-serve)\n- claude-opus-4-5\n- gpt-5.2-codex\n- gemini-3.0-pro-wildly-long\n- claude-haiku-3-5-sonnet".to_string(),
        "1,234".to_string(),
        "56".to_string(),
        "$0.42".to_string(),
    ]);
    let widths = table.column_widths();
    let models_width = widths[1];
    let cell = "- claude-sonnet-4-20250514 (self-serve)\n- claude-opus-4-5\n- gpt-5.2-codex\n- gemini-3.0-pro-wildly-long\n- claude-haiku-3-5-sonnet";
    let widest_line = visible_width_max_line(cell);
    let sum_of_lines = cell.lines().map(visible_width).sum::<usize>();
    assert!(
        models_width < sum_of_lines,
        "Models column width ({models_width}) should be based on widest line ({widest_line}), not sum of all lines ({sum_of_lines})"
    );
    assert!(
        models_width <= widest_line + 3,
        "Models width ({models_width}) should be close to widest line width ({widest_line}), not {sum_of_lines}"
    );
}

#[test]
fn caps_wide_models_without_truncating_large_numeric_values() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input", "Output", "Total Tokens"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120);
    table.push(vec![
        "2026-05-18".to_string(),
        "- provider/this-is-a-deliberately-wide-model-name".to_string(),
        "13,044".to_string(),
        "125,061".to_string(),
        "43,633".to_string(),
    ]);
    table.separator();
    table.push(vec![
        "Total".to_string(),
        String::new(),
        "99,999,999".to_string(),
        "88,888,888".to_string(),
        "77,777,777".to_string(),
    ]);

    let widths = table.column_widths();
    assert_eq!(widths[1], 27);
    assert_eq!(widths[2], 13);
    assert_eq!(widths[3], 13);
    assert_eq!(widths[4], 15);

    let rendered = table.render_lines().join("\n");
    assert!(rendered.contains("13,044"));
    assert!(rendered.contains("99,999,999"));
    assert!(rendered.contains("88,888,888"));
    assert!(rendered.contains("77,777,777"));
    assert!(rendered.contains("43,633"));
}

#[test]
fn caps_models_column_after_agent_column() {
    let mut table = SimpleTable::new(
        vec!["Date", "Agent", "Models", "Input", "Output", "Total Tokens"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Detail,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120);
    table.push(vec![
        "2026-05-18".to_string(),
        "codex".to_string(),
        "- provider/this-is-a-deliberately-wide-model-name".to_string(),
        "13,044".to_string(),
        "125,061".to_string(),
        "43,633".to_string(),
    ]);

    let widths = table.column_widths();
    assert_eq!(widths[2], 27);
    assert!(table.render_lines().join("\n").contains("codex"));
}

#[test]
fn preserves_large_numeric_cells_when_eight_columns_can_fit() {
    let mut table = SimpleTable::new(
        vec![
            "Date",
            "Models",
            "Input",
            "Output",
            "Reasoning",
            "Cache Read",
            "Total Tokens",
            "Cost (USD)",
        ],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120);
    table.push(vec![
        "2026-05-18".to_string(),
        "- provider/this-is-a-deliberately-wide-model-name".to_string(),
        "1,234".to_string(),
        "5,678".to_string(),
        "9,012".to_string(),
        "3,456".to_string(),
        "7,890".to_string(),
        "$12.34".to_string(),
    ]);
    table.separator();
    table.push(vec![
        "Total".to_string(),
        String::new(),
        "99,999,999".to_string(),
        "88,888,888".to_string(),
        "77,777,777".to_string(),
        "66,666,666".to_string(),
        "987,654,321".to_string(),
        "$12345.67".to_string(),
    ]);

    let rendered = table.render_lines().join("\n");
    let total_line = rendered
        .lines()
        .find(|line| line.starts_with("│ Total"))
        .expect("rendered table should include a Total row");
    assert!(!total_line.contains('…'), "{total_line}");
    for value in [
        "99,999,999",
        "88,888,888",
        "77,777,777",
        "66,666,666",
        "987,654,321",
        "$12345.67",
    ] {
        assert!(rendered.contains(value), "missing {value} in {rendered}");
    }
}

#[test]
fn preserves_large_totals_in_the_nine_column_codex_layout_at_120_columns() {
    let mut table = SimpleTable::new(
        vec![
            "Date",
            "Models",
            "Input",
            "Output",
            "Reasoning",
            "Cache Create",
            "Cache Read",
            "Total Tokens",
            "Cost (USD)",
        ],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120)
    .with_date_compaction(true);
    table.push(vec![
        "2026-05-18".to_string(),
        "- provider/this-is-a-deliberately-wide-model-name".to_string(),
        "13,044,466".to_string(),
        "125,061".to_string(),
        "97,285".to_string(),
        "30,366,617".to_string(),
        "43,633,429".to_string(),
        "123,456,789".to_string(),
        "$12.34".to_string(),
    ]);
    table.separator();
    table.push(vec![
        "Total".to_string(),
        String::new(),
        "99,999,999".to_string(),
        "8,888,888".to_string(),
        "777,777".to_string(),
        "66,666,666".to_string(),
        "777,777,777".to_string(),
        "1,048,000,000".to_string(),
        "$123456.78".to_string(),
    ]);

    let widths = table.column_widths();
    assert_eq!(cli_table_required_width(&widths), 120);

    let rendered = table.render_lines().join("\n");
    assert!(
        rendered.lines().all(|line| visible_width(line) == 120),
        "{rendered}"
    );
    let total_line = rendered
        .lines()
        .find(|line| line.starts_with("│ Total"))
        .expect("rendered table should include a Total row");
    assert!(!total_line.contains('…'), "{total_line}");
    assert!(total_line.contains("1,048,000,000"), "{total_line}");
    assert!(total_line.contains("$123456.78"), "{total_line}");
}

#[test]
fn preserves_numeric_cells_at_the_codex_120_column_boundary() {
    let mut table = SimpleTable::new(
        vec![
            "Date",
            "Models",
            "Input",
            "Output",
            "Reasoning",
            "Cache Create",
            "Cache Read",
            "Total Tokens",
            "Cost (USD)",
        ],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120)
    .with_date_compaction(true);
    table.push(vec![
        "2026-05-18".to_string(),
        "- provider/this-is-a-deliberately-wide-model-name".to_string(),
        "123,456,789".to_string(),
        "1,234,567".to_string(),
        "9,876,543".to_string(),
        "1,234,567".to_string(),
        "12,345,678,901".to_string(),
        "12,345,678,901".to_string(),
        "$12345.67".to_string(),
    ]);
    table.separator();
    table.push(vec![
        "Total".to_string(),
        String::new(),
        "123,456,789".to_string(),
        "1,234,567".to_string(),
        "9,876,543".to_string(),
        "1,234,567".to_string(),
        "12,345,678,901".to_string(),
        "12,345,678,901".to_string(),
        "$12345.67".to_string(),
    ]);

    let widths = table.column_widths();
    assert_eq!(cli_table_required_width(&widths), 120);
    assert!(widths[0] < 10 || widths[1] < 12, "{widths:?}");
    assert!(widths[2] >= 13, "{widths:?}");
    assert!(widths[6] >= 16, "{widths:?}");
    assert!(widths[7] >= 16, "{widths:?}");

    let rendered = table.render_lines().join("\n");
    let total_line = rendered
        .lines()
        .find(|line| line.starts_with("│ Total"))
        .expect("rendered table should include a Total row");
    assert!(!total_line.contains('…'), "{total_line}");
    for value in ["123,456,789", "12,345,678,901"] {
        assert!(
            total_line.contains(value),
            "missing {value} in {total_line}"
        );
    }
}

#[test]
fn resets_colored_models_before_each_physical_cell_border() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input"],
        vec![ColumnKind::Primary, ColumnKind::Models, ColumnKind::Numeric],
        TerminalStyle {
            color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120);
    table.push(vec![
        "2026-05-18".to_string(),
        "\x1b[32m- short\n\x1b[32m- provider/this-is-a-deliberately-wide-model-name".to_string(),
        "1,234".to_string(),
    ]);

    let rendered = table.render_lines();
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("- short") || line.contains("provider/this-is"))
            .count(),
        2,
        "{rendered:?}"
    );
    for line in &rendered {
        for cell in line.split('│') {
            let cell = cell.trim_end();
            if cell.contains("\x1b[") {
                assert!(cell.ends_with("\x1b[0m"), "{line:?}");
            }
        }
    }
}

#[test]
fn keeps_date_and_numeric_columns_readable_at_eighty_columns() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input", "Output", "Total Tokens"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(80)
    .with_date_compaction(true);
    table.push(vec![
        "2026-05-18".to_string(),
        "- provider/this-is-a-deliberately-wide-model-name".to_string(),
        "13,044".to_string(),
        "125,061".to_string(),
        "43,633".to_string(),
    ]);
    table.separator();
    table.push(vec![
        "Total".to_string(),
        String::new(),
        "99,999,999".to_string(),
        "88,888,888".to_string(),
        "77,777,777".to_string(),
    ]);

    let rendered = table.render_lines().join("\n");
    assert!(rendered.lines().all(|line| visible_width(line) <= 80));
    assert!(rendered.contains("2026"));
    assert!(rendered.contains("05-18"));
    assert!(rendered.contains("13,044"));
    assert!(rendered.contains("99,999,999"));
    assert!(rendered.contains("88,888,888"));
    assert!(rendered.contains("77,777,777"));
    assert!(!rendered.contains("2026-05-"));
}

#[test]
fn caps_ansi_multiline_models_by_visible_width() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input"],
        vec![ColumnKind::Primary, ColumnKind::Models, ColumnKind::Numeric],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120);
    table.push(vec![
        "2026-05-18".to_string(),
        "\x1b[32m- 表表表表表表表表表表表表表表\x1b[0m\n- short".to_string(),
        "1,234".to_string(),
    ]);

    let widths = table.column_widths();
    assert_eq!(widths[1], 27);

    let rendered = table.render_lines().join("\n");
    assert!(rendered.contains("\x1b[0m…"));
    assert!(rendered.lines().all(|line| visible_width(line) <= 120));
}

#[test]
fn compact_date_layout_remains_stable_with_a_long_model() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input", "Output", "Cost (USD)"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(56)
    .with_date_compaction(true);
    table.push(vec![
        "2026-05-18".to_string(),
        "- provider/this-is-a-deliberately-wide-model-name".to_string(),
        "123,456,789".to_string(),
        "9,876,543".to_string(),
        "$12345.67".to_string(),
    ]);

    let rendered = table.render_lines().join("\n");
    assert!(rendered.contains("2026"));
    assert!(rendered.contains("05-18"));
    assert!(rendered.contains("Models"));
    assert!(rendered.contains("Input"));
    assert!(rendered.contains("Cost"));
}

#[test]
fn preserves_status_markers_when_models_is_not_the_second_column() {
    let mut table = SimpleTable::new(
        vec!["Block Start", "Duration/Status", "Models", "Tokens", "Cost"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Detail,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(56);
    table.push(vec![
        "08/31, 10:00 AM".to_string(),
        "(inactive)".to_string(),
        "- gpt-5".to_string(),
        "123".to_string(),
        "$1.23".to_string(),
    ]);

    let rendered = table.render_lines().join("\n");
    let status_line = rendered
        .lines()
        .find(|line| line.contains("(inactive)"))
        .expect("status marker should remain visible");
    let status_cell = status_line
        .split('│')
        .nth(2)
        .expect("rendered row should include the status cell");
    assert!(!status_cell.contains('…'), "{status_line}");
    assert!(rendered.lines().all(|line| visible_width(line) <= 56));
}

#[test]
fn resets_ansi_continuation_at_explicit_newlines() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input"],
        vec![ColumnKind::Primary, ColumnKind::Models, ColumnKind::Numeric],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(120);
    table.push(vec![
        "2026-05-18".to_string(),
        "\x1b[32m- first\n- second".to_string(),
        "1,234".to_string(),
    ]);

    let rendered = table.render_lines();
    let model_lines = rendered
        .iter()
        .filter(|line| line.contains("- first") || line.contains("- second"))
        .collect::<Vec<_>>();
    assert_eq!(model_lines.len(), 2, "{rendered:?}");
    let first_cell = model_lines[0]
        .split('│')
        .nth(2)
        .expect("rendered row should include the Models cell");
    let second_cell = model_lines[1]
        .split('│')
        .nth(2)
        .expect("rendered row should include the Models cell");
    assert!(first_cell.contains("\x1b[32m"), "{rendered:?}");
    assert!(first_cell.trim_end().ends_with("\x1b[0m"), "{rendered:?}");
    assert!(!second_cell.contains("\x1b[32m"), "{rendered:?}");
}

#[test]
fn keeps_list_markers_attached_to_truncated_models() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input"],
        vec![ColumnKind::Primary, ColumnKind::Models, ColumnKind::Numeric],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(56);
    table.push(vec![
        "2026-05-18".to_string(),
        "- provider/very-long-model-name".to_string(),
        "12345678901".to_string(),
    ]);

    let rendered = table.render_lines().join("\n");
    assert!(
        rendered.lines().any(|line| {
            line.split('│')
                .nth(2)
                .is_some_and(|cell| cell.contains("- provider/"))
        }),
        "{rendered}"
    );
    assert!(!rendered.lines().any(|line| {
        line.split('│')
            .nth(2)
            .is_some_and(|cell| cell.trim() == "-")
    }));
}

#[test]
fn preserves_ansi_continuation_across_word_wrapped_fragments_without_leaking() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input"],
        vec![ColumnKind::Primary, ColumnKind::Models, ColumnKind::Numeric],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(56);
    table.push(vec![
        "2026-05-18".to_string(),
        "\x1b[32m- provider/foo long-model-name\x1b[0m".to_string(),
        "1,234".to_string(),
    ]);

    let rendered = table.render_lines();
    let model_lines = rendered
        .iter()
        .filter(|line| line.contains("provider/foo") || line.contains("long-model-name"))
        .collect::<Vec<_>>();
    assert_eq!(model_lines.len(), 2, "{rendered:?}");
    for line in model_lines {
        let cells = line.split('│').collect::<Vec<_>>();
        let model_cell = cells
            .get(2)
            .expect("rendered row should include the Models cell");
        let input_cell = cells
            .get(3)
            .expect("rendered row should include the Input cell");
        assert!(model_cell.contains("\x1b[32m"), "{line:?}");
        assert!(model_cell.trim_end().ends_with("\x1b[0m"), "{line:?}");
        assert!(!input_cell.contains("\x1b[32m"), "{line:?}");
    }
}

#[test]
fn preserves_ansi_continuation_after_truncation_in_a_narrow_table() {
    let mut table = SimpleTable::new(
        vec!["Date", "Models", "Input", "Output", "Cost (USD)"],
        vec![
            ColumnKind::Primary,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ],
        TerminalStyle {
            no_color: true,
            ..TerminalStyle::default()
        },
    )
    .with_terminal_width(56)
    .with_date_compaction(true);
    table.push(vec![
        "2026-05-18".to_string(),
        "\x1b[32m- provider/very-long-model-name another-model".to_string(),
        "1,234".to_string(),
        "5,678".to_string(),
        "$1.23".to_string(),
    ]);

    let rendered = table.render_lines();
    insta::assert_snapshot!(rendered.join("\n"));
    assert!(
        rendered.iter().all(|line| visible_width(line) == 56),
        "{rendered:?}"
    );
    let model_lines = rendered
        .iter()
        .filter(|line| line.contains("\x1b[32m") || line.contains("another"))
        .collect::<Vec<_>>();
    assert_eq!(model_lines.len(), 2, "{rendered:?}");
    for line in &model_lines {
        let model_cell = line
            .split('│')
            .nth(2)
            .expect("rendered row should include the Models cell");
        assert!(model_cell.contains("\x1b[32m"), "{line:?}");
        assert!(model_cell.trim_end().ends_with("\x1b[0m"), "{line:?}");
    }
    let later_model_cell = model_lines[1]
        .split('│')
        .nth(2)
        .expect("rendered row should include the Models cell");
    assert!(later_model_cell.contains("\x1b[32manother"), "{rendered:?}");
    let later_input_cell = model_lines[1]
        .split('│')
        .nth(3)
        .expect("rendered row should include the Input cell");
    assert!(!later_input_cell.contains("\x1b[32m"), "{rendered:?}");
}
