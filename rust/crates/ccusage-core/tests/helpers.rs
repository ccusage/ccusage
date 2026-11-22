use std::collections::HashMap;

use ccusage_core::{
    cli::WeekDay, cost::tiered_cost, format_number, format_project_name, parse_ts_timestamp,
    week_start,
};

#[test]
fn formats_numbers_with_commas() {
    assert_eq!(format_number(1_234_567), "1,234,567");
}

#[test]
fn calculates_tiered_cost() {
    assert!((tiered_cost(300_000, 3e-6, Some(6e-6), 200_000) - 1.2).abs() < f64::EPSILON);
    assert!((tiered_cost(300_000, 5e-6, Some(10e-6), 272_000) - 1.64).abs() < 1e-9);
}

#[test]
fn formats_windows_user_project_paths() {
    let aliases = HashMap::new();

    assert_eq!(
        format_project_name(r"C:\Users\phaedrus\Development\ccusage", &aliases),
        "ccusage"
    );
    assert_eq!(
        format_project_name(r"\Users\phaedrus\Development\ccusage", &aliases),
        "ccusage"
    );
}

#[test]
fn gets_week_start() {
    assert_eq!(
        week_start("2024-01-03", WeekDay::Sunday).unwrap(),
        "2023-12-31"
    );
    assert_eq!(
        week_start("2024-01-03", WeekDay::Monday).unwrap(),
        "2024-01-01"
    );
}

#[test]
fn parses_timestamp_offsets() {
    assert_eq!(
        parse_ts_timestamp("2024-08-05T08:30:00.000+09:00").unwrap(),
        parse_ts_timestamp("2024-08-04T23:30:00.000Z").unwrap()
    );
    assert_eq!(
        parse_ts_timestamp("2024-08-04T16:30:00-07:00").unwrap(),
        parse_ts_timestamp("2024-08-04T23:30:00Z").unwrap()
    );
}
