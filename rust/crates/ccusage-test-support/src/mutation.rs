use std::{
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind},
};

const REPORTED_FAILURES: usize = 5;

/// Runs `parse` over `fixture` and over every damaged variant of it, and fails
/// the test when any call panics.
///
/// The variants are every prefix of the fixture, every line cut short at every
/// byte with the other lines intact, every JSON value replaced by `null`, and
/// every number and string value swapped for the other type. `parse` only has
/// to return: what it makes of damaged input is not checked.
///
/// # Panics
///
/// Panics with the first few failing variants when `parse` panicked on any.
pub fn assert_parser_survives_mutations(fixture: &[u8], parse: impl Fn(&[u8])) {
    let mut failures = Vec::new();
    let mut failure_count = 0_usize;
    let mut total = 0_usize;
    for mutation in mutations(fixture) {
        total += 1;
        if catch_unwind(AssertUnwindSafe(|| parse(&mutation.bytes))).is_err() {
            failure_count += 1;
            if failures.len() < REPORTED_FAILURES {
                failures.push(mutation);
            }
        }
    }
    assert!(
        failure_count == 0,
        "the parser panicked on {} of {total} damaged inputs; the first are:\n{}",
        failure_count,
        failures
            .iter()
            .map(|mutation| format!(
                "- {}:\n{}",
                mutation.label,
                String::from_utf8_lossy(&mutation.bytes)
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

struct Mutation {
    label: String,
    bytes: Vec<u8>,
}

fn mutations(fixture: &[u8]) -> impl Iterator<Item = Mutation> + '_ {
    let intact = std::iter::once(Mutation {
        label: "the fixture itself".to_string(),
        bytes: fixture.to_vec(),
    });
    let prefixes = (0..fixture.len()).map(|length| Mutation {
        label: format!("truncated to {length} bytes"),
        bytes: fixture[..length].to_vec(),
    });
    let cut_lines = line_ranges(fixture)
        .into_iter()
        .enumerate()
        .flat_map(move |(index, line)| {
            line.clone().map(move |cut| Mutation {
                label: format!("line {} cut to {} bytes", index + 1, cut - line.start),
                bytes: replace(fixture, cut..line.end, b""),
            })
        });
    let values = json_value_spans(fixture)
        .into_iter()
        .flat_map(move |value| value_mutations(fixture, value));
    intact.chain(prefixes).chain(cut_lines).chain(values)
}

fn value_mutations(fixture: &[u8], value: JsonValue) -> Vec<Mutation> {
    let text = String::from_utf8_lossy(&fixture[value.range.clone()]).into_owned();
    let at = value.range.start;
    let mut mutations = Vec::new();
    if text != "null" {
        mutations.push(Mutation {
            label: format!("`{text}` at byte {at} replaced by null"),
            bytes: replace(fixture, value.range.clone(), b"null"),
        });
    }
    match value.kind {
        JsonKind::Number => mutations.push(Mutation {
            label: format!("number `{text}` at byte {at} turned into a string"),
            bytes: replace(fixture, value.range, format!("\"{text}\"").as_bytes()),
        }),
        JsonKind::String => mutations.push(Mutation {
            label: format!("string `{text}` at byte {at} turned into a number"),
            bytes: replace(fixture, value.range, b"1"),
        }),
        JsonKind::Other => {}
    }
    mutations
}

fn replace(fixture: &[u8], range: Range<usize>, replacement: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(fixture.len() + replacement.len());
    bytes.extend_from_slice(&fixture[..range.start]);
    bytes.extend_from_slice(replacement);
    bytes.extend_from_slice(&fixture[range.end..]);
    bytes
}

fn line_ranges(fixture: &[u8]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, byte) in fixture.iter().enumerate() {
        if *byte == b'\n' {
            ranges.push(start..index);
            start = index + 1;
        }
    }
    if start < fixture.len() {
        ranges.push(start..fixture.len());
    }
    ranges
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JsonKind {
    Number,
    String,
    Other,
}

#[derive(Debug, Eq, PartialEq)]
struct JsonValue {
    range: Range<usize>,
    kind: JsonKind,
}

fn json_value_spans(fixture: &[u8]) -> Vec<JsonValue> {
    let mut values = Vec::new();
    let mut open = Vec::new();
    let mut index = 0;
    while index < fixture.len() {
        let start = index;
        match fixture[index] {
            b'"' => {
                index = index_after_string(fixture, index);
                if !is_key(fixture, index) {
                    values.push(JsonValue {
                        range: start..index,
                        kind: JsonKind::String,
                    });
                }
            }
            b'{' | b'[' => {
                open.push(index);
                index += 1;
            }
            b'}' | b']' => {
                index += 1;
                if let Some(start) = open.pop() {
                    values.push(JsonValue {
                        range: start..index,
                        kind: JsonKind::Other,
                    });
                }
            }
            b'-' | b'0'..=b'9' => {
                while index < fixture.len()
                    && matches!(
                        fixture[index],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    index += 1;
                }
                values.push(JsonValue {
                    range: start..index,
                    kind: JsonKind::Number,
                });
            }
            _ => {
                let literal = [&b"true"[..], b"false", b"null"]
                    .into_iter()
                    .find(|literal| fixture[index..].starts_with(literal));
                match literal {
                    Some(literal) => {
                        index += literal.len();
                        values.push(JsonValue {
                            range: start..index,
                            kind: JsonKind::Other,
                        });
                    }
                    None => index += 1,
                }
            }
        }
    }
    values
}

fn index_after_string(fixture: &[u8], start: usize) -> usize {
    let mut index = start + 1;
    while index < fixture.len() {
        match fixture[index] {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    fixture.len()
}

fn is_key(fixture: &[u8], after_string: usize) -> bool {
    fixture[after_string.min(fixture.len())..]
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        == Some(&b':')
}

#[cfg(test)]
mod tests {
    use super::{JsonKind, assert_parser_survives_mutations, json_value_spans, mutations};

    const FIXTURE: &[u8] = b"{\"id\":\"a\\\"b\",\"n\":-1.5e3,\"ok\":true}\n{\"list\":[null, 7]}\n";

    fn mutated() -> Vec<String> {
        mutations(FIXTURE)
            .map(|mutation| String::from_utf8_lossy(&mutation.bytes).into_owned())
            .collect()
    }

    #[test]
    fn finds_values_but_not_keys() {
        let values = json_value_spans(FIXTURE)
            .into_iter()
            .map(|value| {
                (
                    String::from_utf8_lossy(&FIXTURE[value.range]).into_owned(),
                    value.kind,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            values,
            [
                (r#""a\"b""#.to_string(), JsonKind::String),
                ("-1.5e3".to_string(), JsonKind::Number),
                ("true".to_string(), JsonKind::Other),
                (
                    r#"{"id":"a\"b","n":-1.5e3,"ok":true}"#.to_string(),
                    JsonKind::Other
                ),
                ("null".to_string(), JsonKind::Other),
                ("7".to_string(), JsonKind::Number),
                ("[null, 7]".to_string(), JsonKind::Other),
                (r#"{"list":[null, 7]}"#.to_string(), JsonKind::Other),
            ]
        );
    }

    #[test]
    fn covers_every_kind_of_damage() {
        let mutated = mutated();

        for expected in [
            "{\"id\":\"a\\\"b\",\"n\":-1.5e3,\"ok\":true}\n{\"list\":[null, 7]}\n",
            "{\"id\":\"a\\\"b\",\"n\":-1.5",
            "{\"id\":\n{\"list\":[null, 7]}\n",
            "{\"id\":\"a\\\"b\",\"n\":null,\"ok\":true}\n{\"list\":[null, 7]}\n",
            "{\"id\":\"a\\\"b\",\"n\":\"-1.5e3\",\"ok\":true}\n{\"list\":[null, 7]}\n",
            "{\"id\":1,\"n\":-1.5e3,\"ok\":true}\n{\"list\":[null, 7]}\n",
            "{\"id\":\"a\\\"b\",\"n\":-1.5e3,\"ok\":true}\n{\"list\":null}\n",
        ] {
            assert!(
                mutated.iter().any(|bytes| bytes == expected),
                "missing {expected:?}"
            );
        }
    }

    #[test]
    fn passes_a_parser_that_always_returns() {
        assert_parser_survives_mutations(FIXTURE, |bytes| {
            let _ = std::str::from_utf8(bytes);
        });
    }

    #[test]
    #[should_panic(expected = "the parser panicked on")]
    fn reports_a_parser_that_panics_on_damaged_input() {
        assert_parser_survives_mutations(FIXTURE, |bytes| {
            let first_line = bytes
                .split(|byte| *byte == b'\n')
                .next()
                .unwrap_or_default();
            assert!(first_line.is_empty() || first_line.len() > 3);
        });
    }
}
