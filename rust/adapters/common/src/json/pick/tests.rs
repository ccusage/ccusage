use std::{borrow::Cow, collections::BTreeMap};

use serde::{Deserialize, de::IgnoredAny};
use serde_json::Value;

use super::{Fields, Member, Picked};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Loose<'a> {
    #[serde(borrow)]
    timestamp: Option<Cow<'a, str>>,
    message: Option<Message>,
    session_id: Option<String>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    is_sidechain: Option<bool>,
    data: Option<Data<'a>>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Strict<'a> {
    #[serde(borrow)]
    timestamp: Cow<'a, str>,
    message: Message,
    #[serde(default)]
    session_id: Option<Option<&'a str>>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Message {
    usage: Option<Value>,
    model: Option<String>,
    id: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Data<'a> {
    #[serde(borrow)]
    message: Option<Inner<'a>>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Inner<'a> {
    #[serde(borrow)]
    timestamp: Option<&'a str>,
    message: Message,
}

const MESSAGE: Fields = &[
    ("usage", Member::Value),
    ("model", Member::Value),
    ("id", Member::Value),
];

const LINE: Fields = &[
    ("timestamp", Member::Value),
    ("message", Member::Object(MESSAGE)),
    ("sessionId", Member::Value),
    ("costUSD", Member::Value),
    ("isSidechain", Member::Value),
    (
        "data",
        Member::Object(&[(
            "message",
            Member::Object(&[
                ("timestamp", Member::Value),
                ("message", Member::Object(MESSAGE)),
            ]),
        )]),
    ),
];

fn same<'a, T>(text: &'a str, picked: &Picked<'a>)
where
    T: Deserialize<'a> + PartialEq + std::fmt::Debug,
{
    match (serde_json::from_str::<T>(text), picked.parse::<T>()) {
        (Ok(expected), Ok(actual)) => assert_eq!(expected, actual, "{text:?}"),
        (Err(_), Err(_)) => {}
        (expected, actual) => panic!(
            "{text:?}: full parse {:?}, picked {:?}",
            expected.map(drop),
            actual.map(drop)
        ),
    }
}

fn check(text: &str) -> bool {
    let Some(picked) = Picked::new(text, LINE) else {
        return false;
    };
    same::<Loose<'_>>(text, &picked);
    same::<Strict<'_>>(text, &picked);
    same::<IgnoredAny>(text, &picked);
    assert!(
        serde_json::from_str::<IgnoredAny>(text).is_ok(),
        "picked a line the full parse rejects: {text:?}"
    );
    true
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const STRING_PIECES: &[&str] = &[
    "a",
    "message",
    "usage",
    "é",
    "😀",
    r"\n",
    r#"\""#,
    r"\\",
    r#"\\\""#,
    r"\\\\",
    r"\/",
    r"A",
    r"😀",
    r"\uD800",
    r"\uDC00",
    r"\u12",
    r"\u00zz",
    r"\x",
    "\u{1}",
    "\t",
    "\u{7f}",
    r#"{"usage":{"#,
    r#"","message":{"id":""#,
    " ",
];

const KEYS: &[&str] = &[
    "timestamp",
    "message",
    "usage",
    "model",
    "id",
    "sessionId",
    "costUSD",
    "isSidechain",
    "data",
    "content",
    "type",
    r"timestamp",
    r"message",
    r"model",
    r"sessionId",
    r"\uD800ey",
    r"key\x",
    r#"\"message"#,
    r"message\\",
    "message",
];

const WHITESPACE: &[&str] = &["", "", "", " ", "\n", "\t", "\r\n", "  ", "\u{c}"];

fn string(rng: &mut Rng, out: &mut String) {
    out.push('"');
    for _ in 0..rng.below(6) {
        out.push_str(rng.pick(STRING_PIECES));
    }
    out.push('"');
}

fn space(rng: &mut Rng, out: &mut String) {
    if rng.below(4) == 0 {
        out.push_str(rng.pick(WHITESPACE));
    }
}

fn object(rng: &mut Rng, depth: usize, out: &mut String) {
    out.push('{');
    for index in 0..rng.below(6) {
        if index > 0 {
            out.push(',');
        }
        space(rng, out);
        out.push('"');
        out.push_str(rng.pick(KEYS));
        out.push('"');
        space(rng, out);
        out.push(':');
        space(rng, out);
        value(rng, depth + 1, out);
        space(rng, out);
    }
    out.push('}');
}

fn value(rng: &mut Rng, depth: usize, out: &mut String) {
    match rng.below(if depth > 4 { 4 } else { 8 }) {
        0 | 1 => string(rng, out),
        2 => out.push_str(rng.pick(&[
            "12",
            "-3",
            "4.5e1",
            "0",
            "-0",
            "1E+2",
            "18446744073709551615",
            "18446744073709551616",
            "01",
            "1.",
            "-",
            ".5",
            "1e",
            "2.5E-3",
        ])),
        3 => out.push_str(rng.pick(&["true", "false", "null", "nul", "True"])),
        4..=6 => object(rng, depth, out),
        _ => {
            out.push('[');
            for index in 0..rng.below(4) {
                if index > 0 {
                    out.push(',');
                }
                space(rng, out);
                value(rng, depth + 1, out);
            }
            out.push(']');
        }
    }
}

fn corrupt(rng: &mut Rng, line: &mut String) {
    for _ in 0..rng.below(3) {
        if line.is_empty() {
            return;
        }
        let mut at = rng.below(line.len() + 1);
        while !line.is_char_boundary(at) {
            at -= 1;
        }
        match rng.below(8) {
            0 => line.truncate(at),
            1 if at < line.len() => {
                line.remove(at);
            }
            2 => line.insert(
                at,
                rng.pick(&["\"", ",", ":", "{", "}", "[", "]", "\\", " "])
                    .chars()
                    .next()
                    .unwrap(),
            ),
            _ => {}
        }
    }
}

fn deep(levels: usize) -> String {
    format!(
        r#"{{"timestamp":"t","content":{}1{},"message":{{"id":"m"}}}}"#,
        "[".repeat(levels),
        "]".repeat(levels)
    )
}

#[test]
fn matches_the_full_parse_on_generated_lines() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let cases = 300_000;
    let (mut valid, mut picked) = (0, 0);
    for _ in 0..cases {
        let mut line = String::new();
        space(&mut rng, &mut line);
        object(&mut rng, 0, &mut line);
        space(&mut rng, &mut line);
        if rng.below(2) == 0 {
            corrupt(&mut rng, &mut line);
        }
        let took = check(&line);
        if serde_json::from_str::<IgnoredAny>(&line).is_ok()
            && !line.trim().contains(['\t', '\r', '\n', '\u{c}'])
        {
            valid += 1;
            picked += usize::from(took);
        }
    }
    assert!(
        picked * 100 >= valid * 95,
        "only {picked} of {valid} valid lines took the picked path"
    );
}

#[test]
fn matches_the_full_parse_on_written_edge_cases() {
    for line in [
        r#"{"timestamp":"t","message":{"id":"a","id":"b"}}"#,
        r#"{"timestamp":"t","message":{"id":"a"},"message":{"id":"b"}}"#,
        r#"{"timestamp":"t","message":{"content":"x","id":"a"},"content":"y","content":"z"}"#,
        r#"{"timestamp":"t","message":{"id":"a"}}"#,
        r#"{"timestamp":"t","message":{"id":"a\\"},"x":"\\\"","y":"\\\\"}"#,
        r#"{"timestamp":"t","message":{"content":[{"message":{"id":"inner"}}],"id":"outer"}}"#,
        r#"{"timestamp":"t","message":["u","m","i"]}"#,
        r#"{"timestamp":"t","message":null}"#,
        r#"{"timestamp":"\uD800","message":{}}"#,
        r#"{"timestamp":"t","message":{},"x":"\uD800"}"#,
        r#"{"timestamp":"t","message":{"id":"a"}} "#,
        " {\"timestamp\":\"t\",\"message\":{\"id\":\"a\"}}\r",
        r#"{"timestamp":"t","message":{"id":"a"}}x"#,
        r#"{"timestamp":"t","message":{"id":"a"},}"#,
        r#"{"data":{"message":{"timestamp":"t","message":{"id":"p"},"content":"c"}},"timestamp":"t","message":{}}"#,
        &deep(10),
        &deep(70),
        &deep(130),
    ] {
        check(line);
    }
}

#[test]
fn falls_back_where_it_cannot_match_the_full_parse_cheaply() {
    for line in [
        "{\"timestamp\":\"a\tb\",\"message\":{}}",
        "{\"timestamp\":\"t\",\t\"message\":{}}",
        r#"{"\uD800":"t","message":{}}"#,
        &deep(70),
        "[1]",
        "",
    ] {
        assert!(Picked::new(line, LINE).is_none(), "{line:?}");
    }
}

#[test]
fn skips_long_strings_with_escape_runs_before_quotes() {
    let content = format!("{}\\\\\\\" more \\\\", "x".repeat(10_000));
    let line = format!(r#"{{"message":{{"content":"{content}","id":"a"}},"timestamp":"t"}}"#);
    assert!(check(&line));
    let picked = Picked::new(&line, LINE).unwrap();
    let strict = picked.parse::<Strict<'_>>().unwrap();
    assert_eq!(strict.message.id.as_deref(), Some("a"));
}

#[test]
fn keeps_only_the_picked_members() {
    let line = r#"{"message":{"content":"x","id":"a"},"other":1,"timestamp":"t"}"#;
    let keys = Picked::new(line, LINE)
        .unwrap()
        .parse::<BTreeMap<String, IgnoredAny>>()
        .unwrap();
    assert_eq!(
        keys.into_keys().collect::<Vec<_>>(),
        ["message", "timestamp"]
    );
}

#[test]
fn leaves_invalid_utf8_to_the_full_parse() {
    let bytes = b"{\"timestamp\":\"t\",\"message\":{\"content\":\"\xff\",\"id\":\"a\"}}";
    assert!(crate::json::JsonText::new(bytes).pick(LINE).is_none());
}

#[test]
fn finds_a_control_byte_in_any_lane() {
    for len in 0..80 {
        for at in 0..len {
            for byte in (0..=u8::MAX).step_by(7).chain([0x1f, 0x20, 0x7f, 0x80]) {
                let mut bytes = vec![b'a'; len];
                bytes[at] = byte;
                assert_eq!(
                    super::has_control_byte(&bytes),
                    byte < 0x20,
                    "{len} {at} {byte}"
                );
            }
        }
    }
}
