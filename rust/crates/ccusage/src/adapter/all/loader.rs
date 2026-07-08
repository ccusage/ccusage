use std::{collections::BTreeMap, sync::mpsc, thread};

use serde_json::{Value, json};

use crate::{
    CodexGroup, LoadedEntry, ModelBreakdown, PricingMap, Result, SessionAccumulator, UsageSummary,
    adapter::{
        amp, claude, codebuff, codex, copilot, droid, gemini, goose, hermes, kilo, kimi, openclaw,
        opencode, pi, qwen,
    },
    cli::{AgentReportKind, CodexSpeed, SharedArgs, WeekDay},
    filter_loaded_entries_by_date, json_float,
};

use super::{
    report::sort_rows,
    types::{AgentLoadSpec, AgentRows, AllAccumulator, AllLoadResult, AllRow, LoadedAgentRows},
};

pub(super) fn load_rows(kind: AgentReportKind, shared: &SharedArgs) -> Result<AllLoadResult> {
    let mut progress = crate::progress::UsageLoadProgress::new(
        crate::log_level() != Some(0)
            && crate::progress::should_show_usage_load_progress(
                shared.json,
                crate::progress::usage_load_output_is_tty(),
            ),
    );
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let load_kind = match kind {
        AgentReportKind::Session => AgentReportKind::Session,
        AgentReportKind::Daily | AgentReportKind::Weekly | AgentReportKind::Monthly => {
            AgentReportKind::Daily
        }
    };
    let loader_shared = SharedArgs {
        json: true,
        ..shared.clone()
    };
    let loaded = load_agent_rows_parallel(
        vec![
            AgentLoadSpec {
                index: 0,
                agent: "claude",
                progress_agent: crate::progress::UsageLoadAgent::Claude,
                load: Box::new(|| load_claude_rows(load_kind, &loader_shared)),
            },
            AgentLoadSpec {
                index: 1,
                agent: "codex",
                progress_agent: crate::progress::UsageLoadAgent::Codex,
                load: Box::new(|| load_codex_rows(load_kind, &loader_shared, &pricing)),
            },
            AgentLoadSpec {
                index: 2,
                agent: "opencode",
                progress_agent: crate::progress::UsageLoadAgent::OpenCode,
                load: Box::new(|| {
                    load_summary_agent_rows(
                        "opencode",
                        load_kind,
                        &loader_shared,
                        || opencode::loader::load_entries(&loader_shared),
                        opencode::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 3,
                agent: "amp",
                progress_agent: crate::progress::UsageLoadAgent::Amp,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "amp",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        amp::load_entries,
                        amp::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 4,
                agent: "droid",
                progress_agent: crate::progress::UsageLoadAgent::Droid,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "droid",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        droid::load_entries,
                        droid::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 5,
                agent: "codebuff",
                progress_agent: crate::progress::UsageLoadAgent::Codebuff,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "codebuff",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        codebuff::load_entries,
                        codebuff::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 6,
                agent: "hermes",
                progress_agent: crate::progress::UsageLoadAgent::Hermes,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "hermes",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        hermes::load_entries,
                        hermes::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 7,
                agent: "pi",
                progress_agent: crate::progress::UsageLoadAgent::Pi,
                load: Box::new(|| {
                    load_session_capable_summary_agent_rows(
                        "pi",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        pi::load_entries,
                        pi::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 8,
                agent: "goose",
                progress_agent: crate::progress::UsageLoadAgent::Goose,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "goose",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        goose::load_entries,
                        goose::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 9,
                agent: "openclaw",
                progress_agent: crate::progress::UsageLoadAgent::OpenClaw,
                load: Box::new(|| {
                    load_summary_agent_rows(
                        "openclaw",
                        load_kind,
                        &loader_shared,
                        || openclaw::load_entries(&loader_shared, None, Some(&pricing)),
                        openclaw::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 10,
                agent: "kilo",
                progress_agent: crate::progress::UsageLoadAgent::Kilo,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "kilo",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        kilo::load_entries,
                        kilo::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 11,
                agent: "copilot",
                progress_agent: crate::progress::UsageLoadAgent::Copilot,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "copilot",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        copilot::load_entries,
                        copilot::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 12,
                agent: "gemini",
                progress_agent: crate::progress::UsageLoadAgent::Gemini,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "gemini",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        gemini::load_entries,
                        gemini::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 13,
                agent: "kimi",
                progress_agent: crate::progress::UsageLoadAgent::Kimi,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "kimi",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        kimi::load_entries,
                        kimi::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 14,
                agent: "qwen",
                progress_agent: crate::progress::UsageLoadAgent::Qwen,
                load: Box::new(|| load_qwen_rows(load_kind, &loader_shared)),
            },
        ],
        &mut progress,
    )?;
    let mut detected_agents = Vec::new();
    let mut rows = Vec::new();
    for loaded in loaded {
        append_agent_rows(
            &mut rows,
            &mut detected_agents,
            loaded.agent,
            loaded.agent_rows,
        );
    }
    if kind == AgentReportKind::Session {
        for row in &mut rows {
            row.metadata_agents = None;
        }
        sort_rows(&mut rows, &shared.order);
        return Ok(AllLoadResult {
            rows,
            detected_agents,
        });
    }

    let mut aggregated = aggregate_rows(rows, kind);
    sort_rows(&mut aggregated, &shared.order);
    Ok(AllLoadResult {
        rows: aggregated,
        detected_agents,
    })
}

pub(super) fn load_agent_rows_parallel(
    specs: Vec<AgentLoadSpec<'_>>,
    progress: &mut crate::progress::UsageLoadProgress,
) -> Result<Vec<LoadedAgentRows>> {
    for spec in &specs {
        progress.start(spec.progress_agent);
    }

    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let mut handles = Vec::with_capacity(specs.len());
        for spec in specs {
            let sender = sender.clone();
            handles.push((
                spec.index,
                spec.progress_agent,
                scope.spawn(move || {
                    let result = (spec.load)();
                    let _ = sender.send((spec.index, spec.agent, spec.progress_agent, result));
                }),
            ));
        }
        drop(sender);

        let mut loaded = Vec::with_capacity(handles.len());
        let mut errors = Vec::new();
        for (index, agent, progress_agent, result) in receiver {
            match result {
                Ok(agent_rows) => {
                    progress.succeed(progress_agent);
                    loaded.push(LoadedAgentRows {
                        index,
                        agent,
                        agent_rows,
                    });
                }
                Err(error) => {
                    progress.fail(progress_agent);
                    errors.push((index, error));
                }
            }
        }

        for (index, progress_agent, handle) in handles {
            if handle.join().is_err() {
                progress.fail(progress_agent);
                errors.push((index, crate::cli_error("agent loader panicked")));
            }
        }

        errors.sort_by_key(|(index, _)| *index);
        if let Some((_, error)) = errors.into_iter().next() {
            return Err(error);
        }

        loaded.sort_by_key(|loaded| loaded.index);
        Ok(loaded)
    })
}

fn append_agent_rows(
    rows: &mut Vec<AllRow>,
    detected_agents: &mut Vec<&'static str>,
    agent: &'static str,
    agent_rows: AgentRows,
) {
    if agent_rows.detected {
        detected_agents.push(agent);
    }
    rows.extend(agent_rows.rows);
}

fn load_summary_agent_rows(
    agent: &'static str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    load_entries: impl FnOnce() -> Result<Vec<LoadedEntry>>,
    summarize_entries: impl FnOnce(&[LoadedEntry], AgentReportKind) -> Result<Vec<UsageSummary>>,
) -> Result<AgentRows> {
    let mut entries = load_entries()?;
    let detected = !entries.is_empty();
    filter_loaded_entries_by_date(&mut entries, shared);
    let summaries = summarize_entries(&entries, kind)?;
    Ok(AgentRows {
        rows: summary_rows(agent, summaries),
        detected,
    })
}

fn load_session_capable_summary_agent_rows(
    agent: &'static str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
    load_entries: impl FnOnce(
        &SharedArgs,
        Option<&str>,
        Option<&PricingMap>,
    ) -> Result<Vec<LoadedEntry>>,
    summarize_entries: impl FnOnce(&[LoadedEntry], AgentReportKind) -> Result<Vec<UsageSummary>>,
) -> Result<AgentRows> {
    let mut entries = load_entries(shared, None, Some(pricing))?;
    let detected = !entries.is_empty();
    let summaries = if kind == AgentReportKind::Session {
        filter_loaded_entries_by_date(&mut entries, shared);
        summarize_entry_sessions(&entries)?
    } else {
        filter_loaded_entries_by_date(&mut entries, shared);
        summarize_entries(&entries, kind)?
    };
    Ok(AgentRows {
        rows: summary_rows(agent, summaries),
        detected,
    })
}

fn load_claude_rows(kind: AgentReportKind, shared: &SharedArgs) -> Result<AgentRows> {
    if kind == AgentReportKind::Session {
        let entries = claude::load_entries(shared, None)?;
        let detected = !entries.is_empty();
        let mut summaries = summarize_entry_sessions(&entries)?;
        filter_session_summaries(&mut summaries, shared);
        return Ok(AgentRows {
            rows: summary_rows("claude", summaries),
            detected,
        });
    }

    let mut summaries = claude::load_daily_summaries(shared, None, false)?;
    let detected = !summaries.is_empty();
    filter_daily_summaries_by_date(&mut summaries, shared);
    Ok(AgentRows {
        rows: summary_rows("claude", summaries),
        detected,
    })
}

fn filter_daily_summaries_by_date(rows: &mut Vec<UsageSummary>, shared: &SharedArgs) {
    if shared.since.is_none() && shared.until.is_none() {
        return;
    }
    rows.retain(|row| {
        let date = row.date.as_deref().unwrap_or_default().replace('-', "");
        shared.since.as_ref().is_none_or(|since| &date >= since)
            && shared.until.as_ref().is_none_or(|until| &date <= until)
    });
}

fn load_codex_rows(
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    if shared.since.is_none() && shared.until.is_none() {
        let groups = codex::load_groups(shared, kind)?;
        let detected = !groups.is_empty();
        let speed = codex::resolve_codex_speed(CodexSpeed::Auto);
        return Ok(AgentRows {
            rows: groups
                .iter()
                .map(|(period, group)| codex_group_row(period, group, pricing, speed))
                .collect(),
            detected,
        });
    }

    let mut events = codex::load_codex_events(shared)?;
    let detected = !events.is_empty();
    codex::filter_events_by_date(&mut events, shared)?;
    let groups = codex::aggregate_events(&events, kind, shared.timezone.as_deref())?;
    let speed = codex::resolve_codex_speed(CodexSpeed::Auto);
    Ok(AgentRows {
        rows: groups
            .iter()
            .map(|(period, group)| codex_group_row(period, group, pricing, speed))
            .collect(),
        detected,
    })
}

fn load_priced_summary_agent_rows(
    agent: &'static str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
    load_entries: impl FnOnce(&SharedArgs, &PricingMap) -> Result<Vec<LoadedEntry>>,
    summarize_entries: impl FnOnce(&[LoadedEntry], AgentReportKind) -> Result<Vec<UsageSummary>>,
) -> Result<AgentRows> {
    load_summary_agent_rows(
        agent,
        kind,
        shared,
        || load_entries(shared, pricing),
        summarize_entries,
    )
}

fn load_qwen_rows(kind: AgentReportKind, shared: &SharedArgs) -> Result<AgentRows> {
    let mut entries = qwen::load_entries(shared)?;
    let detected = !entries.is_empty() || qwen::has_data();
    if kind == AgentReportKind::Session {
        let mut summaries = qwen::summarize_entries(&entries, kind)?;
        filter_session_summaries(&mut summaries, shared);
        return Ok(AgentRows {
            rows: summary_rows("qwen", summaries),
            detected,
        });
    }
    filter_loaded_entries_by_date(&mut entries, shared);
    let summaries = qwen::summarize_entries(&entries, kind)?;
    Ok(AgentRows {
        rows: summary_rows("qwen", summaries),
        detected,
    })
}

fn summarize_entry_sessions(entries: &[LoadedEntry]) -> Result<Vec<UsageSummary>> {
    let mut groups = BTreeMap::<(String, String), SessionAccumulator>::new();
    for entry in entries {
        groups
            .entry((entry.project_path.to_string(), entry.session_id.to_string()))
            .or_default()
            .add_entry(entry);
    }
    groups
        .into_values()
        .map(|group| group.into_summary())
        .collect()
}

fn filter_session_summaries(rows: &mut Vec<UsageSummary>, shared: &SharedArgs) {
    if shared.since.is_some() || shared.until.is_some() {
        rows.retain(|row| {
            let date = row
                .last_activity
                .as_deref()
                .unwrap_or_default()
                .replace('-', "");
            shared.since.as_ref().is_none_or(|since| &date >= since)
                && shared.until.as_ref().is_none_or(|until| &date <= until)
        });
    }
}

fn summary_rows(agent: &'static str, summaries: Vec<UsageSummary>) -> Vec<AllRow> {
    summaries
        .into_iter()
        .filter_map(|summary| {
            let period = summary
                .date
                .as_ref()
                .or(summary.week.as_ref())
                .or(summary.month.as_ref())
                .or(summary.session_id.as_ref())?
                .clone();
            let total_tokens = summary.total_tokens();
            if total_tokens == 0 {
                return None;
            }
            let metadata = summary_metadata(agent, &summary);
            Some(AllRow {
                period,
                agent,
                models_used: summary.models_used,
                input_tokens: summary.input_tokens,
                output_tokens: summary.output_tokens,
                cache_creation_tokens: summary.cache_creation_tokens,
                cache_read_tokens: summary.cache_read_tokens,
                total_tokens,
                total_cost: summary.total_cost,
                metadata,
                metadata_agents: Some(vec![agent]),
                agent_breakdowns: None,
                model_breakdowns: summary.model_breakdowns,
            })
        })
        .collect()
}

fn summary_metadata(agent: &'static str, summary: &UsageSummary) -> Option<Value> {
    let mut metadata = serde_json::Map::new();
    if let Some(credits) = summary.credits {
        metadata.insert("credits".to_string(), json_float(credits));
    }
    if summary.session_id.is_some() {
        if let Some(last_activity) = summary.last_activity.as_ref() {
            metadata.insert("lastActivity".to_string(), json!(last_activity));
        }
        if agent == "pi"
            && let Some(project_path) = summary.project_path.as_ref()
        {
            metadata.insert("projectPath".to_string(), json!(project_path));
        }
    }
    if metadata.is_empty() {
        None
    } else {
        Some(Value::Object(metadata))
    }
}

pub(super) fn codex_group_row(
    period: &str,
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> AllRow {
    let mut model_breakdowns: Vec<ModelBreakdown> = group
        .models
        .iter()
        .map(|(model, usage)| {
            let input =
                codex::non_cached_input_tokens(usage.input_tokens, usage.cached_input_tokens);
            ModelBreakdown {
                model_name: model.clone(),
                input_tokens: input,
                output_tokens: usage.output_tokens,
                cache_creation_tokens: 0,
                cache_read_tokens: usage.cached_input_tokens,
                extra_total_tokens: 0,
                cost: codex::calculate_codex_model_cost(model, usage, pricing, speed),
                missing_pricing: codex::codex_model_missing_pricing(model, usage, pricing),
            }
        })
        .collect();
    model_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    AllRow {
        period: period.to_string(),
        agent: "codex",
        models_used: group.models.keys().cloned().collect(),
        input_tokens: codex::non_cached_input_tokens(group.input_tokens, group.cached_input_tokens),
        output_tokens: group.output_tokens,
        cache_creation_tokens: 0,
        cache_read_tokens: group.cached_input_tokens,
        total_tokens: group.total_tokens,
        total_cost: codex::calculate_group_cost(group, pricing, speed),
        metadata: Some(json!({
            "lastActivity": group.last_activity,
            "reasoningOutputTokens": group.reasoning_output_tokens,
        })),
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns,
    }
}

pub(super) fn aggregate_rows(rows: Vec<AllRow>, kind: AgentReportKind) -> Vec<AllRow> {
    let mut groups = BTreeMap::<String, AllAccumulator>::new();
    for mut row in rows {
        let period = match kind {
            AgentReportKind::Daily => row.period.clone(),
            AgentReportKind::Monthly => row
                .period
                .get(..7)
                .map_or_else(|| row.period.clone(), str::to_string),
            AgentReportKind::Weekly => crate::week_start(&row.period, WeekDay::Monday)
                .unwrap_or_else(|| row.period.clone()),
            AgentReportKind::Session => row.period.clone(),
        };
        row.period = period.clone();
        groups.entry(period).or_default().add(row);
    }
    groups
        .into_iter()
        .map(|(period, group)| group.into_row(period))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    fn usage_summary(date: &str, input_tokens: u64) -> UsageSummary {
        UsageSummary {
            date: Some(date.to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            total_cost: 0.0,
            credits: None,
            message_count: None,
            models_used: Vec::new(),
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }
    }

    #[test]
    fn filters_daily_summaries_with_compact_date_bounds() {
        let mut rows = vec![
            usage_summary("2026-01-01", 10),
            usage_summary("2026-01-02", 20),
            usage_summary("2026-01-03", 30),
        ];
        let shared = SharedArgs {
            since: Some("20260102".to_string()),
            until: Some("20260102".to_string()),
            ..SharedArgs::default()
        };

        filter_daily_summaries_by_date(&mut rows, &shared);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date.as_deref(), Some("2026-01-02"));
        assert_eq!(rows[0].input_tokens, 20);
    }

    fn usage_fingerprint(rows: &[AllRow]) -> Vec<(String, u64, u64, u64, u64, u64)> {
        rows.iter()
            .map(|row| {
                (
                    row.period.clone(),
                    row.input_tokens,
                    row.output_tokens,
                    row.cache_creation_tokens,
                    row.cache_read_tokens,
                    row.total_tokens,
                )
            })
            .collect()
    }

    fn pi_path_subcommand_rows(
        store_path: &str,
        shared: &SharedArgs,
        pricing: &PricingMap,
    ) -> Result<Vec<AllRow>> {
        let mut entries = pi::load_entries(shared, Some(store_path), Some(pricing))?;
        filter_loaded_entries_by_date(&mut entries, shared);
        let summaries = pi::summarize_entries(&entries, AgentReportKind::Session)?;
        Ok(summary_rows("pi", summaries))
    }

    #[test]
    fn default_pi_unified_session_window_keeps_until_day_like_pi_path() {
        let fixture = fs_fixture!({
            "pi/sessions/project-a/agent_until-day.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "pi/sessions/project-a/agent_after-day.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
        });
        let _pi_agent_dir = EnvVarGuard::set("PI_AGENT_DIR", fixture.path("pi/sessions"));
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("America/Boise".to_string()),
            since: Some("20260704".to_string()),
            until: Some("20260704".to_string()),
            ..SharedArgs::default()
        };

        let result = load_rows(AgentReportKind::Session, &shared).unwrap();
        let pi_rows = result
            .rows
            .iter()
            .filter(|row| row.agent == "pi")
            .cloned()
            .collect::<Vec<_>>();
        let pi_path_rows = pi_path_subcommand_rows(
            fixture.path("pi/sessions").to_str().unwrap(),
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(
            usage_fingerprint(&pi_rows),
            usage_fingerprint(&pi_path_rows)
        );
        assert_eq!(
            usage_fingerprint(&pi_rows),
            vec![("until-day".to_string(), 10, 1, 0, 0, 11)]
        );
    }
}
