use super::*;
use crate::TimestampMs;

const CHILD: &str = "child";
const PARENT: &str = "parent";
const PARENT_LEN: usize = 3;

#[derive(Clone, Copy, Debug)]
enum Line {
    Count,
    OwnSettings,
    ForeignSettings,
    OwnRecord,
    UntaggedRecord,
}

const LINES: [Line; 5] = [
    Line::Count,
    Line::OwnSettings,
    Line::ForeignSettings,
    Line::OwnRecord,
    Line::UntaggedRecord,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parent {
    Missing,
    Unrelated,
    ReplayedAsPrefix,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    spawned: bool,
    child_known: bool,
    parent: Parent,
}

fn usage(input_tokens: u64) -> CodexRawUsage {
    CodexRawUsage {
        input_tokens,
        total_tokens: input_tokens,
        ..CodexRawUsage::default()
    }
}

fn event(input_tokens: u64) -> CodexTokenUsageEvent {
    CodexTokenUsageEvent {
        session_id: CHILD.to_string(),
        response_id: None,
        record_response_id: None,
        timestamp: TimestampMs::from_millis(0),
        last_activity: String::new(),
        model: "gpt-5".to_string(),
        input_tokens,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: input_tokens,
        is_fallback_model: false,
        service_tier: None,
    }
}

fn parent_usage(parent: Parent) -> Vec<CodexRawUsage> {
    match parent {
        Parent::Missing => Vec::new(),
        Parent::Unrelated => (1..=PARENT_LEN as u64)
            .map(|value| usage(value * 7))
            .collect(),
        Parent::ReplayedAsPrefix => (0..PARENT_LEN as u64)
            .map(|value| usage(100 + value))
            .collect(),
    }
}

/// Input tokens of each count line, in order: the `n`-th plain count
/// replays the parent's `n`-th usage when the parent is replayed as a
/// prefix, and every other count is unique.
fn values(lines: &[Line], parent: Parent) -> Vec<u64> {
    let mut plain = 0;
    let mut values = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        match line {
            Line::Count => {
                values.push(
                    if parent == Parent::ReplayedAsPrefix && plain < PARENT_LEN {
                        100 + plain as u64
                    } else {
                        1_000 + index as u64
                    },
                );
                plain += 1;
            }
            Line::OwnRecord => values.push(5_000 + index as u64),
            Line::OwnSettings | Line::ForeignSettings | Line::UntaggedRecord => {}
        }
    }
    values
}

fn run(lines: &[Line], case: Case) -> (Vec<u64>, bool) {
    let parent = parent_usage(case.parent);
    let mut filter = CodexReplayFilter::new(Some(CodexForkReplay {
        parent_usage: &parent,
        missing_parent: (case.parent == Parent::Missing).then_some(PARENT),
        child_id: case.child_known.then_some(CHILD),
        ends_at_usage_record: case.spawned,
    }));
    let mut values = values(lines, case.parent).into_iter();
    let mut emitted = Vec::new();
    let mut warned = false;
    let mut visit = |event: CodexTokenUsageEvent| emitted.push(event.input_tokens);
    let mut note = |resolution: Option<CodexReplayResolution<'_>>| {
        warned |= matches!(resolution, Some(CodexReplayResolution::MissingParent(_)));
    };
    for line in lines {
        match line {
            Line::Count => filter.visit(event(values.next().unwrap()), &mut visit),
            Line::OwnSettings => note(filter.observe_marker(
                CodexOwnershipMarker::ThreadSettings,
                Some(CHILD),
                &mut visit,
            )),
            Line::ForeignSettings => note(filter.observe_marker(
                CodexOwnershipMarker::ThreadSettings,
                Some(PARENT),
                &mut visit,
            )),
            Line::OwnRecord => {
                note(filter.observe_marker(
                    CodexOwnershipMarker::UsageRecord,
                    Some(CHILD),
                    &mut visit,
                ));
                filter.visit(event(values.next().unwrap()), &mut visit);
            }
            Line::UntaggedRecord => {
                note(filter.observe_marker(CodexOwnershipMarker::UsageRecord, None, &mut visit))
            }
        }
    }
    note(filter.finish(&mut visit));
    (emitted, warned)
}

/// The rule as `rust/adapters/codex/src/README.md` states it, written
/// over whole lines rather than the filter's streaming state.
fn expected(lines: &[Line], case: Case) -> (Vec<u64>, bool) {
    let values = values(lines, case.parent);
    let is_own =
        |line: &Line| case.child_known && matches!(line, Line::OwnSettings | Line::OwnRecord);
    let resolution = lines
        .iter()
        .position(|line| {
            (case.child_known && matches!(line, Line::OwnRecord))
                || (case.spawned && matches!(line, Line::UntaggedRecord))
        })
        .unwrap_or(lines.len());
    let boundary = lines.iter().take(resolution + 1).position(is_own);
    let foreign = boundary.and_then(|boundary| {
        lines[..boundary]
            .iter()
            .position(|line| case.child_known && matches!(line, Line::ForeignSettings))
    });
    let leading_end = foreign.or(boundary).unwrap_or(resolution);
    let mut counted = Vec::new();
    let mut leading = Vec::new();
    let mut values = values.into_iter();
    for (index, line) in lines.iter().enumerate() {
        if !matches!(line, Line::Count | Line::OwnRecord) {
            continue;
        }
        let value = values.next().unwrap();
        let copied = matches!((foreign, boundary), (Some(f), Some(b)) if f < index && index < b);
        if index < leading_end {
            leading.push(value);
        } else if !copied {
            counted.push(value);
        }
    }
    let skipped = match case.parent {
        Parent::Missing | Parent::Unrelated => 0,
        Parent::ReplayedAsPrefix => leading
            .iter()
            .zip(100..)
            .take(PARENT_LEN)
            .take_while(|(value, parent)| **value == *parent)
            .count(),
    };
    let warned = case.parent == Parent::Missing && !leading.is_empty();
    let mut emitted = leading.split_off(skipped);
    emitted.append(&mut counted);
    (emitted, warned)
}

fn sequences(max_len: usize) -> Vec<Vec<Line>> {
    let mut all = vec![Vec::new()];
    let mut frontier = vec![Vec::new()];
    for _ in 0..max_len {
        frontier = frontier
            .iter()
            .flat_map(|prefix: &Vec<Line>| {
                LINES.iter().map(move |line| {
                    let mut next = prefix.clone();
                    next.push(*line);
                    next
                })
            })
            .collect();
        all.extend(frontier.iter().cloned());
    }
    all
}

#[test]
fn filter_follows_the_documented_fork_rule_for_every_short_rollout() {
    let mut failures = Vec::new();
    for lines in sequences(6) {
        for spawned in [false, true] {
            for child_known in [false, true] {
                for parent in [Parent::Missing, Parent::Unrelated, Parent::ReplayedAsPrefix] {
                    let case = Case {
                        spawned,
                        child_known,
                        parent,
                    };
                    let actual = run(&lines, case);
                    let expected = expected(&lines, case);
                    if actual != expected {
                        failures.push(format!(
                            "{lines:?} {case:?}: got {actual:?}, documented {expected:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} cases differ, first: {:#?}",
        failures.len(),
        &failures[..failures.len().min(10)]
    );
}

#[test]
fn parent_matching_finds_the_longest_prefix_or_suffix() {
    let alphabet = [1_u64, 2, 3];
    let words = |max: usize| {
        let mut words = vec![Vec::new()];
        let mut frontier = vec![Vec::new()];
        for _ in 0..max {
            frontier = frontier
                .iter()
                .flat_map(|prefix: &Vec<u64>| {
                    alphabet.iter().map(move |value| {
                        let mut next = prefix.clone();
                        next.push(*value);
                        next
                    })
                })
                .collect();
            words.extend(frontier.iter().cloned());
        }
        words
    };
    let words = words(5);
    for parent in &words {
        let parent = parent.iter().copied().map(usage).collect::<Vec<_>>();
        for child in &words {
            let child = child.iter().copied().map(usage).collect::<Vec<_>>();
            let naive = (0..=parent.len().min(child.len()))
                .filter(|len| {
                    parent[..*len] == child[..*len] || parent[parent.len() - len..] == child[..*len]
                })
                .max()
                .unwrap_or(0);
            assert_eq!(
                replayed_parent_len(&parent, &child),
                naive,
                "parent {parent:?}, child {child:?}"
            );
        }
    }
}
