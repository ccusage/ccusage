use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use serde_json::{Value, json};

use crate::{
    CodexGroup, LoadedEntry, ModelBreakdown, PricingMap, Result, SessionAccumulator, UsageSummary,
    adapter::{
        amp, claude, codebuff, codex, copilot, droid, gemini, goose, hermes, kilo, kimi, openclaw,
        opencode, pi, qwen,
    },
    cli::{AgentReportKind, CodexSpeed, NamedPiStore, SharedArgs, WeekDay},
    filter_loaded_entries_by_date, json_float,
};

use super::{
    report::sort_rows,
    types::{
        AgentLoadSpec, AgentRows, AllAccumulator, AllLoadResult, AllRow, AllSectionsLoadResult,
        LoadedAgentRows,
    },
};

pub(crate) const BUILT_IN_AGENT_NAMES: &[&str] = &[
    "claude", "codex", "opencode", "amp", "droid", "codebuff", "hermes", "pi", "goose", "openclaw",
    "kilo", "copilot", "gemini", "kimi", "qwen",
];

pub(super) fn load_rows(kind: AgentReportKind, shared: &SharedArgs) -> Result<AllLoadResult> {
    let pricing = load_pricing(shared);
    let load_kind = load_kind_for_report(kind);
    let loaded = load_base_rows(load_kind, shared, &pricing)?;
    Ok(AllLoadResult {
        rows: finish_rows(kind, loaded.rows, shared),
        detected_agents: loaded.detected_agents,
    })
}

pub(super) fn load_sections(
    kinds: &[AgentReportKind],
    shared: &SharedArgs,
) -> Result<AllSectionsLoadResult> {
    let pricing = load_pricing(shared);
    let daily_base = needs_daily_family(kinds)
        .then(|| load_base_rows(AgentReportKind::Daily, shared, &pricing))
        .transpose()?;
    let session_base = needs_session(kinds)
        .then(|| load_base_rows(AgentReportKind::Session, shared, &pricing))
        .transpose()?;

    let daily_detected_agents = daily_base
        .as_ref()
        .map(|base| base.detected_agents.clone())
        .unwrap_or_default();
    let session_detected_agents = session_base
        .as_ref()
        .map(|base| base.detected_agents.clone())
        .unwrap_or_default();

    let mut sections = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let rows = if *kind == AgentReportKind::Session {
            session_base
                .as_ref()
                .map(|base| finish_rows(*kind, base.rows.clone(), shared))
                .unwrap_or_default()
        } else {
            daily_base
                .as_ref()
                .map(|base| finish_rows(*kind, base.rows.clone(), shared))
                .unwrap_or_default()
        };
        sections.push((*kind, rows));
    }

    Ok(AllSectionsLoadResult {
        sections,
        daily_detected_agents,
        session_detected_agents,
    })
}

fn load_pricing(shared: &SharedArgs) -> PricingMap {
    PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    )
}

fn load_kind_for_report(kind: AgentReportKind) -> AgentReportKind {
    match kind {
        AgentReportKind::Session => AgentReportKind::Session,
        AgentReportKind::Daily | AgentReportKind::Weekly | AgentReportKind::Monthly => {
            AgentReportKind::Daily
        }
    }
}

fn load_base_rows(
    load_kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AllLoadResult> {
    let mut progress = crate::progress::UsageLoadProgress::new(
        crate::log_level() != Some(0)
            && crate::progress::should_show_usage_load_progress(
                shared.json,
                crate::progress::usage_load_output_is_tty(),
            ),
    );
    let loader_shared = SharedArgs {
        json: true,
        ..shared.clone()
    };
    let mut specs = vec![
        AgentLoadSpec {
            index: 0,
            agent: BUILT_IN_AGENT_NAMES[0],
            progress_agent: crate::progress::UsageLoadAgent::Claude,
            load: Box::new(|| load_claude_rows(load_kind, &loader_shared)),
        },
        AgentLoadSpec {
            index: 1,
            agent: BUILT_IN_AGENT_NAMES[1],
            progress_agent: crate::progress::UsageLoadAgent::Codex,
            load: Box::new(|| load_codex_rows(load_kind, &loader_shared, pricing)),
        },
        AgentLoadSpec {
            index: 2,
            agent: BUILT_IN_AGENT_NAMES[2],
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
            agent: BUILT_IN_AGENT_NAMES[3],
            progress_agent: crate::progress::UsageLoadAgent::Amp,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "amp",
                    load_kind,
                    &loader_shared,
                    pricing,
                    amp::load_entries,
                    amp::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 4,
            agent: BUILT_IN_AGENT_NAMES[4],
            progress_agent: crate::progress::UsageLoadAgent::Droid,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "droid",
                    load_kind,
                    &loader_shared,
                    pricing,
                    droid::load_entries,
                    droid::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 5,
            agent: BUILT_IN_AGENT_NAMES[5],
            progress_agent: crate::progress::UsageLoadAgent::Codebuff,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "codebuff",
                    load_kind,
                    &loader_shared,
                    pricing,
                    codebuff::load_entries,
                    codebuff::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 6,
            agent: BUILT_IN_AGENT_NAMES[6],
            progress_agent: crate::progress::UsageLoadAgent::Hermes,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "hermes",
                    load_kind,
                    &loader_shared,
                    pricing,
                    hermes::load_entries,
                    hermes::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 7,
            agent: BUILT_IN_AGENT_NAMES[7],
            progress_agent: crate::progress::UsageLoadAgent::Pi,
            load: Box::new(|| {
                load_pi_format_agent_rows("pi", None, load_kind, &loader_shared, pricing)
            }),
        },
        AgentLoadSpec {
            index: 8,
            agent: BUILT_IN_AGENT_NAMES[8],
            progress_agent: crate::progress::UsageLoadAgent::Goose,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "goose",
                    load_kind,
                    &loader_shared,
                    pricing,
                    goose::load_entries,
                    goose::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 9,
            agent: BUILT_IN_AGENT_NAMES[9],
            progress_agent: crate::progress::UsageLoadAgent::OpenClaw,
            load: Box::new(|| {
                load_summary_agent_rows(
                    "openclaw",
                    load_kind,
                    &loader_shared,
                    || openclaw::load_entries(&loader_shared, None, Some(pricing)),
                    openclaw::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 10,
            agent: BUILT_IN_AGENT_NAMES[10],
            progress_agent: crate::progress::UsageLoadAgent::Kilo,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "kilo",
                    load_kind,
                    &loader_shared,
                    pricing,
                    kilo::load_entries,
                    kilo::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 11,
            agent: BUILT_IN_AGENT_NAMES[11],
            progress_agent: crate::progress::UsageLoadAgent::Copilot,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "copilot",
                    load_kind,
                    &loader_shared,
                    pricing,
                    copilot::load_entries,
                    copilot::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 12,
            agent: BUILT_IN_AGENT_NAMES[12],
            progress_agent: crate::progress::UsageLoadAgent::Gemini,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "gemini",
                    load_kind,
                    &loader_shared,
                    pricing,
                    gemini::load_entries,
                    gemini::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 13,
            agent: BUILT_IN_AGENT_NAMES[13],
            progress_agent: crate::progress::UsageLoadAgent::Kimi,
            load: Box::new(|| {
                load_priced_summary_agent_rows(
                    "kimi",
                    load_kind,
                    &loader_shared,
                    pricing,
                    kimi::load_entries,
                    kimi::summarize_entries,
                )
            }),
        },
        AgentLoadSpec {
            index: 14,
            agent: BUILT_IN_AGENT_NAMES[14],
            progress_agent: crate::progress::UsageLoadAgent::Qwen,
            load: Box::new(|| load_qwen_rows(load_kind, &loader_shared)),
        },
    ];
    let named_pi_stores = resolve_named_pi_store_paths(&shared.pi_stores)?;
    for store in named_pi_stores {
        let agent = leak_agent_name(&store.name);
        let paths = store.paths;
        let index = specs.len();
        let loader_shared_ref = &loader_shared;
        let pricing_ref = pricing;
        specs.push(AgentLoadSpec {
            index,
            agent,
            progress_agent: crate::progress::UsageLoadAgent::PiStore(agent),
            load: Box::new(move || {
                load_named_pi_store_rows_from_paths(
                    agent,
                    paths,
                    load_kind,
                    loader_shared_ref,
                    pricing_ref,
                )
            }),
        });
    }
    let loaded = load_agent_rows_parallel(specs, &mut progress)?;
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
    Ok(AllLoadResult {
        rows,
        detected_agents,
    })
}

fn finish_rows(kind: AgentReportKind, mut rows: Vec<AllRow>, shared: &SharedArgs) -> Vec<AllRow> {
    if kind == AgentReportKind::Session {
        for row in &mut rows {
            row.metadata_agents = None;
        }
        sort_rows(&mut rows, &shared.order);
        return rows;
    }

    let mut aggregated = aggregate_rows(rows, kind);
    sort_rows(&mut aggregated, &shared.order);
    aggregated
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

fn leak_agent_name(name: &str) -> &'static str {
    Box::leak(name.to_string().into_boxed_str())
}

struct ResolvedNamedPiStore {
    name: String,
    paths: Vec<PathBuf>,
}

fn resolve_named_pi_store_paths(stores: &[NamedPiStore]) -> Result<Vec<ResolvedNamedPiStore>> {
    let mut owners = BTreeMap::<PathBuf, String>::new();
    for path in pi::default_paths(None)? {
        owners.insert(path_key(&path), "default pi store".to_string());
    }

    let mut resolved = Vec::with_capacity(stores.len());
    for store in stores {
        let mut paths = Vec::new();
        let mut collision_owners = BTreeSet::new();
        for path in pi::named_store_paths(&store.path)? {
            let key = path_key(&path);
            if let Some(owner) = owners
                .iter()
                .find(|(owned, _)| paths_overlap(&key, owned))
                .map(|(_, owner)| owner.clone())
            {
                collision_owners.insert(owner);
                continue;
            }
            owners.insert(key, format!("pi.stores name '{}'", store.name));
            paths.push(path);
        }

        if !collision_owners.is_empty() {
            return Err(crate::cli_error(format!(
                "Invalid ccusage config: pi.stores name '{}' paths overlap {}",
                store.name,
                collision_owners.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }

        resolved.push(ResolvedNamedPiStore {
            name: store.name.clone(),
            paths,
        });
    }
    Ok(resolved)
}

fn path_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Session files are collected recursively, so a store rooted at an ancestor
/// of another store (or of the default pi store) would ingest the same files
/// twice under different dedupe identities. Treat equal and nested paths as
/// overlapping.
fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
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
        rows: summary_rows(agent, summaries, false),
        detected,
    })
}

fn load_pi_format_agent_rows(
    agent: &'static str,
    custom_path: Option<&str>,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    let mut entries = pi::load_entries(shared, custom_path, Some(pricing))?;
    let detected = !entries.is_empty();
    let summaries = if kind == AgentReportKind::Session {
        filter_loaded_entries_by_date(&mut entries, shared);
        summarize_entry_sessions(&entries)?
    } else {
        filter_loaded_entries_by_date(&mut entries, shared);
        pi::summarize_entries(&entries, kind)?
    };
    Ok(AgentRows {
        rows: summary_rows(agent, summaries, true),
        detected,
    })
}

#[cfg(test)]
fn load_named_pi_store_rows(
    agent: &'static str,
    store_path: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    let entries = pi::load_entries_for_store_path(shared, store_path, agent, Some(pricing))?;
    filtered_pi_format_agent_rows(agent, kind, shared, entries, true)
}

fn load_named_pi_store_rows_from_paths(
    agent: &'static str,
    store_paths: Vec<PathBuf>,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    let entries = pi::load_entries_for_store_paths(shared, store_paths, agent, Some(pricing))?;
    filtered_pi_format_agent_rows(agent, kind, shared, entries, true)
}

fn filtered_pi_format_agent_rows(
    agent: &'static str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    mut entries: Vec<LoadedEntry>,
    include_project_path: bool,
) -> Result<AgentRows> {
    let detected = !entries.is_empty();
    filter_loaded_entries_by_date(&mut entries, shared);
    let summaries = pi::summarize_entries(&entries, kind)?;
    Ok(AgentRows {
        rows: summary_rows(agent, summaries, include_project_path),
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
            rows: summary_rows("claude", summaries, false),
            detected,
        });
    }

    let mut summaries = claude::load_daily_summaries(shared, None, false)?;
    let detected = !summaries.is_empty();
    filter_daily_summaries_by_date(&mut summaries, shared);
    Ok(AgentRows {
        rows: summary_rows("claude", summaries, false),
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
            rows: summary_rows("qwen", summaries, false),
            detected,
        });
    }
    filter_loaded_entries_by_date(&mut entries, shared);
    let summaries = qwen::summarize_entries(&entries, kind)?;
    Ok(AgentRows {
        rows: summary_rows("qwen", summaries, false),
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

fn needs_daily_family(kinds: &[AgentReportKind]) -> bool {
    kinds.iter().any(|kind| *kind != AgentReportKind::Session)
}

fn needs_session(kinds: &[AgentReportKind]) -> bool {
    kinds.contains(&AgentReportKind::Session)
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

fn summary_rows(
    agent: &'static str,
    summaries: Vec<UsageSummary>,
    include_project_path: bool,
) -> Vec<AllRow> {
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
            let metadata = summary_metadata(&summary, include_project_path);
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

fn summary_metadata(summary: &UsageSummary, include_project_path: bool) -> Option<Value> {
    let mut metadata = serde_json::Map::new();
    if let Some(credits) = summary.credits {
        metadata.insert("credits".to_string(), json_float(credits));
    }
    if summary.session_id.is_some() {
        if let Some(last_activity) = summary.last_activity.as_ref() {
            metadata.insert("lastActivity".to_string(), json!(last_activity));
        }
        if include_project_path && let Some(project_path) = summary.project_path.as_ref() {
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
    use ccusage_cli::NamedPiStore;
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

    fn pi_path_subcommand_rows(
        store_path: &str,
        kind: AgentReportKind,
        shared: &SharedArgs,
        pricing: &PricingMap,
    ) -> Result<Vec<AllRow>> {
        let mut entries = pi::load_entries(shared, Some(store_path), Some(pricing))?;
        filter_loaded_entries_by_date(&mut entries, shared);
        let summaries = pi::summarize_entries(&entries, kind)?;
        Ok(summary_rows("pi", summaries, true))
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

    #[test]
    fn load_rows_filters_named_pi_store_sessions_like_pi_path_subcommand() {
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_inside-session.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "omp/sessions/project-a/agent_outside-session.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
        });
        let _pi_agent_dir = EnvVarGuard::set("PI_AGENT_DIR", fixture.path("empty-default"));
        let store_path = fixture.path("omp/sessions").to_string_lossy().into_owned();
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("America/Boise".to_string()),
            since: Some("20260704".to_string()),
            until: Some("20260704".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: store_path.clone(),
            }],
            ..SharedArgs::default()
        };

        let result = load_rows(AgentReportKind::Session, &shared).unwrap();
        let omp_rows = result
            .rows
            .iter()
            .filter(|row| row.agent == "omp")
            .collect::<Vec<_>>();
        let pi_rows = pi_path_subcommand_rows(
            &store_path,
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(omp_rows.len(), 1);
        assert_eq!(omp_rows[0].period, "inside-session");
        assert_eq!(omp_rows[0].input_tokens, 10);
        assert_eq!(omp_rows[0].output_tokens, 1);
        assert_eq!(
            usage_fingerprint(&pi_rows),
            vec![("inside-session".to_string(), 10, 1, 0, 0, 11)]
        );
    }

    #[test]
    fn named_pi_store_windowed_period_rows_match_pi_path_subcommand() {
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_july-third.jsonl": r#"{"type":"message","timestamp":"2026-07-03T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "omp/sessions/project-a/agent_july-fourth.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
            "omp/sessions/project-a/agent_july-fifth.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":3}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            timezone: Some("America/Boise".to_string()),
            since: Some("20260703".to_string()),
            until: Some("20260704".to_string()),
            ..SharedArgs::default()
        };
        let store_path = fixture.path("omp/sessions").to_string_lossy().into_owned();
        let pricing = PricingMap::default();
        let cases = [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
        ];

        for kind in cases {
            let named_rows = load_named_pi_store_rows("omp", &store_path, kind, &shared, &pricing)
                .unwrap()
                .rows;
            let pi_rows = pi_path_subcommand_rows(&store_path, kind, &shared, &pricing).unwrap();

            assert_eq!(usage_fingerprint(&named_rows), usage_fingerprint(&pi_rows));
        }
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
            AgentReportKind::Session,
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

    #[test]
    fn default_pi_session_rows_keep_main_project_path_derivation() {
        let fixture = fs_fixture!({
            "default-root/archive/sessions/project-a/agent_default-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let rows = load_pi_format_agent_rows(
            "pi",
            Some(fixture.path("default-root").to_str().unwrap()),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(rows.rows.len(), 1);
        assert_eq!(rows.rows[0].agent, "pi");
        assert_eq!(
            rows.rows[0].metadata.as_ref().unwrap()["projectPath"],
            json!("project-a")
        );
    }

    #[test]
    fn named_pi_store_rows_keep_agent_model_and_project_metadata() {
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let rows = load_named_pi_store_rows(
            "omp",
            fixture.path("omp/sessions").to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert!(rows.detected);
        assert_eq!(rows.rows.len(), 1);
        assert_eq!(rows.rows[0].agent, "omp");
        assert_eq!(rows.rows[0].models_used, vec!["[omp] gpt-5"]);
        assert_eq!(
            rows.rows[0].metadata.as_ref().unwrap()["projectPath"],
            json!("project-a")
        );
    }

    #[test]
    fn named_pi_store_rows_derive_project_relative_to_store_root() {
        let fixture = fs_fixture!({
            "store/project-b/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
            "backups/sessions/omp-copy/project-c/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-03T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let plain_root = load_named_pi_store_rows(
            "omp",
            fixture.path("store").to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();
        let earlier_sessions_root = load_named_pi_store_rows(
            "omp",
            fixture.path("backups/sessions/omp-copy").to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(
            plain_root.rows[0].metadata.as_ref().unwrap()["projectPath"],
            json!("project-b")
        );
        assert_eq!(
            earlier_sessions_root.rows[0].metadata.as_ref().unwrap()["projectPath"],
            json!("project-c")
        );
    }

    #[test]
    fn named_pi_store_absent_path_is_clean_empty() {
        let fixture = fs_fixture!({});
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let rows = load_named_pi_store_rows(
            "omp",
            fixture.path("missing/sessions").to_str().unwrap(),
            AgentReportKind::Daily,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert!(!rows.detected);
        assert!(rows.rows.is_empty());
    }

    #[test]
    fn load_rows_rejects_named_pi_store_that_collides_with_default_pi_path() {
        let fixture = fs_fixture!({
            ".pi/agent/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        });
        let _pi_agent_dir = EnvVarGuard::set("PI_AGENT_DIR", fixture.path(".pi/agent/sessions"));
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture
                    .path(".pi/agent/sessions")
                    .to_string_lossy()
                    .into_owned(),
            }],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(AgentReportKind::Daily, &shared) else {
            panic!("expected default pi path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("omp"));
        assert!(error.to_string().contains("default pi"));
    }

    #[test]
    fn load_rows_rejects_named_pi_store_that_collides_with_another_store() {
        let fixture = fs_fixture!({
            "shared/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        });
        let _pi_agent_dir = EnvVarGuard::set("PI_AGENT_DIR", fixture.path("empty-default"));
        let path = fixture
            .path("shared/sessions")
            .to_string_lossy()
            .into_owned();
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![
                NamedPiStore {
                    name: "omp".to_string(),
                    path: path.clone(),
                },
                NamedPiStore {
                    name: "fork".to_string(),
                    path,
                },
            ],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(AgentReportKind::Daily, &shared) else {
            panic!("expected named pi store path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("fork"));
        assert!(error.to_string().contains("omp"));
    }

    #[test]
    fn load_rows_rejects_named_pi_store_nested_inside_default_pi_path() {
        let fixture = fs_fixture!({
            ".pi/agent/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        });
        let _pi_agent_dir = EnvVarGuard::set("PI_AGENT_DIR", fixture.path(".pi/agent/sessions"));
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture
                    .path(".pi/agent/sessions/project-a")
                    .to_string_lossy()
                    .into_owned(),
            }],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(AgentReportKind::Daily, &shared) else {
            panic!("expected nested default pi path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("omp"));
        assert!(error.to_string().contains("default pi"));
    }

    #[test]
    fn load_rows_rejects_named_pi_store_partially_overlapping_another_store() {
        let fixture = fs_fixture!({
            "shared/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        });
        let _pi_agent_dir = EnvVarGuard::set("PI_AGENT_DIR", fixture.path("empty-default"));
        let shared_path = fixture
            .path("shared/sessions")
            .to_string_lossy()
            .into_owned();
        let distinct = fixture.create_dir_all("fork-only/sessions");
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![
                NamedPiStore {
                    name: "omp".to_string(),
                    path: shared_path.clone(),
                },
                NamedPiStore {
                    name: "fork".to_string(),
                    path: format!("{}, {}", distinct.display(), shared_path),
                },
            ],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(AgentReportKind::Daily, &shared) else {
            panic!("expected partial named pi store path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("fork"));
        assert!(error.to_string().contains("omp"));
    }

    #[test]
    fn load_rows_includes_named_pi_store_through_production_wiring() {
        let fixture = fs_fixture!({
            "omp/sessions/project-a/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
        });
        let _pi_agent_dir = EnvVarGuard::set("PI_AGENT_DIR", fixture.path("empty-default"));
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("UTC".to_string()),
            since: Some("20990102".to_string()),
            until: Some("20990102".to_string()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture.path("omp/sessions").to_string_lossy().into_owned(),
            }],
            ..SharedArgs::default()
        };

        let result = load_rows(AgentReportKind::Daily, &shared).unwrap();

        assert!(result.detected_agents.contains(&"omp"));
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].metadata_agents, Some(vec!["omp"]));
        assert_eq!(result.rows[0].models_used, vec!["[omp] gpt-5"]);
        assert_eq!(result.rows[0].input_tokens, 30);
        assert_eq!(result.rows[0].output_tokens, 40);
    }

    #[test]
    fn named_pi_store_rows_are_additive_to_default_pi_rows() {
        let fixture = fs_fixture!({
            "pi/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
            "omp/sessions/project-b/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };
        let mut rows = load_pi_format_agent_rows(
            "pi",
            Some(fixture.path("pi/sessions").to_str().unwrap()),
            AgentReportKind::Daily,
            &shared,
            &PricingMap::default(),
        )
        .unwrap()
        .rows;
        rows.extend(
            load_named_pi_store_rows(
                "omp",
                fixture.path("omp/sessions").to_str().unwrap(),
                AgentReportKind::Daily,
                &shared,
                &PricingMap::default(),
            )
            .unwrap()
            .rows,
        );

        let aggregated = aggregate_rows(rows, AgentReportKind::Daily);

        assert_eq!(aggregated.len(), 1);
        assert_eq!(aggregated[0].metadata_agents, Some(vec!["omp", "pi"]));
        let breakdowns = aggregated[0].agent_breakdowns.as_ref().unwrap();
        assert_eq!(
            breakdowns
                .iter()
                .map(|row| (row.agent, row.models_used.as_slice()))
                .collect::<Vec<_>>(),
            vec![
                ("omp", ["[omp] gpt-5".to_string()].as_slice()),
                ("pi", ["[pi] gpt-5".to_string()].as_slice()),
            ]
        );
    }
}
