use std::fs;

use crate::adapter::claude::paths::{claude_paths, usage_files};
use crate::cli::{SharedArgs, SkillsArgs};
use crate::pricing::PricingMap;

use self::attribute::{Attribution, attribute};
use self::dedup::dedup;
use self::dimensions::{dimensions, long_session_share};
use self::record::parse_records;
use self::report::{build_report, report_json};
use self::thread::{build_thread, link_subagents};

pub(crate) mod record;
pub(crate) mod dedup;
pub(crate) mod thread;
pub(crate) mod attribute;
pub(crate) mod dimensions;
pub(crate) mod report;
#[cfg(test)] mod tests_corpus;

pub(crate) fn run_skills(args: SkillsArgs) -> crate::Result<()> {
    let paths = claude_paths()?;
    let files = usage_files(&paths, None);

    let mut main_files = Vec::new();
    let mut sub_files = Vec::new();
    for f in files {
        if f.components().any(|c| c.as_os_str() == "subagents") {
            sub_files.push(f);
        } else {
            main_files.push(f);
        }
    }

    let pricing = Some(PricingMap::load_with_overrides(
        args.shared.offline,
        false,
        args.shared.pricing_overrides.iter(),
    ));

    let mut all_records = Vec::new();
    let mut graphs = Vec::new();
    let mut session_totals: Vec<(f64, f64)> = Vec::new();

    for mf in &main_files {
        let Ok(content) = fs::read(mf) else { continue };
        let recs = dedup(parse_records(&content));
        let (span, toks) = span_and_tokens(&recs);
        session_totals.push((span, toks));
        all_records.extend(recs.iter().cloned());

        let main_thread = build_thread(&recs, mf.to_string_lossy().into_owned(), false);
        let stem = mf.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let mut subs = Vec::new();
        for sf in &sub_files {
            if sf.to_string_lossy().contains(stem) {
                if let Ok(c) = fs::read(sf) {
                    let sr = dedup(parse_records(&c));
                    all_records.extend(sr.iter().cloned());
                    subs.push((
                        sf.to_string_lossy().into_owned(),
                        build_thread(&sr, sf.to_string_lossy().into_owned(), true),
                    ));
                }
            }
        }
        graphs.push(link_subagents(main_thread, subs));
    }

    let mut merged = Attribution::default();
    for g in &graphs {
        let a = attribute(g);
        merged.skills.extend(a.skills);
        merged.baseline.input += a.baseline.input;
        merged.baseline.output += a.baseline.output;
        merged.baseline.cache_creation += a.baseline.cache_creation;
        merged.baseline.cache_read += a.baseline.cache_read;
    }

    let model = all_records
        .iter()
        .rev()
        .find_map(|r| r.model.clone())
        .unwrap_or_else(|| "claude-opus-4-8".to_string());
    let dims = dimensions(&all_records, &merged.skills);
    let long = long_session_share(&session_totals, args.min_hours);

    let mut report = build_report(&merged, &dims, &model, pricing.as_ref());
    report.long_session_share = long;

    if crate::wants_json(&args.shared) {
        crate::print_json_or_jq(
            report_json(&report),
            args.shared.jq.as_deref(),
            args.shared.no_cost,
        )?;
    } else {
        print_table(&report, long, args.min_hours, &args.shared);
    }
    Ok(())
}

fn span_and_tokens(recs: &[record::Record]) -> (f64, f64) {
    let mut toks = 0.0;
    let mut first: Option<String> = None;
    let mut last: Option<String> = None;
    for r in recs {
        if let Some(u) = r.usage {
            toks += (u.input_tokens
                + u.output_tokens
                + u.cache_creation_token_count()
                + u.cache_read_input_tokens) as f64;
        }
        if let Some(ts) = &r.timestamp {
            if first.is_none() {
                first = Some(ts.clone());
            }
            last = Some(ts.clone());
        }
    }
    let span = match (first, last) {
        (Some(a), Some(b)) => crate::parse_ts_timestamp(&b)
            .zip(crate::parse_ts_timestamp(&a))
            .map(|(b, a)| (b.as_millis() - a.as_millis()) as f64 / 3_600_000.0)
            .unwrap_or(0.0),
        _ => 0.0,
    };
    (span, toks)
}

fn print_table(report: &report::SkillsReport, long_share: f64, min_hours: f64, shared: &SharedArgs) {
    crate::print_box_title("Claude Code Skill Attribution", shared);
    println!("model: {}", report.model);
    println!(
        "{:<40} {:>14} {:>14} {:>10} {:>10}",
        "skill", "excl.tokens", "incl.tokens", "excl.$", "incl.$"
    );
    for s in &report.skills {
        println!(
            "{:<40} {:>14} {:>14} {:>10.4} {:>10.4}",
            truncate(&s.name, 40),
            s.exclusive_tokens,
            s.inclusive_tokens,
            s.exclusive_cost,
            s.inclusive_cost
        );
    }
    println!("baseline (no skill): {} tokens", report.baseline_tokens);
    println!();
    println!(
        "high-context (>150k) share: {:.1}%",
        report.high_context_share * 100.0
    );
    println!(
        "subagent share:             {:.1}%",
        report.subagent_share * 100.0
    );
    println!("long-session (>={min_hours:.0}h) share: {:.1}%", long_share * 100.0);
    println!("\nby plugin:");
    for (p, t) in &report.plugin_tokens {
        println!("  {:<24} {:>14.0}", p, t);
    }
    println!("\nnote: inclusive double-counts by design (nesting + subagents); exclusive reconciles to billed.");
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}
