use ccusage_adapter_common::json::{Fields, JsonText, Member};
use serde::{
    Deserialize, Deserializer,
    de::{Error as _, Visitor},
    forward_to_deserialize_any,
};

use super::{
    AgentProgressData, AgentProgressEntry, AgentProgressMessage, RawUsageMessage, RawUsageRecord,
    USAGE_LINE, USAGE_MESSAGE, parse_full_usage_line, parse_usage_line,
};

struct FieldNames(Option<&'static [&'static str]>);

impl<'de> Deserializer<'de> for &mut FieldNames {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(Self::Error::custom("not a struct"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0 = Some(fields);
        Err(Self::Error::custom("probed"))
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
        identifier ignored_any
    }
}

fn field_names<'de, T: Deserialize<'de>>() -> Vec<&'static str> {
    let mut probe = FieldNames(None);
    let _ = T::deserialize(&mut probe);
    probe.0.expect("a derived struct").to_vec()
}

fn picked_names(fields: Fields) -> Vec<&'static str> {
    fields.iter().map(|(name, _)| *name).collect()
}

fn inner(fields: Fields, name: &str) -> Fields {
    match fields.iter().find(|(field, _)| *field == name) {
        Some((_, Member::Object(inner))) => inner,
        _ => panic!("{name} is not reduced"),
    }
}

#[test]
fn picks_every_member_the_record_types_read() {
    let top = picked_names(USAGE_LINE);
    for name in field_names::<RawUsageRecord<'_>>()
        .into_iter()
        .chain(field_names::<AgentProgressEntry<'_>>())
    {
        assert!(top.contains(&name), "{name}");
    }
    assert_eq!(
        picked_names(USAGE_MESSAGE),
        field_names::<RawUsageMessage>()
    );
    let data = inner(USAGE_LINE, "data");
    assert_eq!(picked_names(data), field_names::<AgentProgressData<'_>>());
    let progress = inner(data, "message");
    assert_eq!(
        picked_names(progress),
        field_names::<AgentProgressMessage<'_>>()
    );
    assert!(std::ptr::eq(inner(progress, "message"), USAGE_MESSAGE));
    assert!(std::ptr::eq(inner(USAGE_LINE, "message"), USAGE_MESSAGE));
}

struct Rng {
    state: u64,
    noise: usize,
}

impl Rng {
    fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const TEXT: &[&str] = &[
    "plain words ",
    "日本語",
    "😀",
    r"\n",
    r"\t",
    r#"\""#,
    r"\\",
    r#"\\\""#,
    r"\\\\",
    r"\u0041",
    r"\uD83D\uDE00",
    r"\uD800",
    r#"\",\"usage\":{\"input_tokens\":9"#,
    "}]}",
    r#"","message":{"id":"fake"},"x":""#,
    r#"{"usage":{"#,
    r#"","usage":null,"x":""#,
];

fn text(rng: &mut Rng, out: &mut String, pieces: usize) {
    out.push('"');
    let usable = if rng.chance(rng.noise) {
        TEXT.len()
    } else {
        TEXT.len() - 3
    };
    for _ in 0..rng.below(pieces) {
        out.push_str(TEXT[rng.below(usable)]);
    }
    out.push('"');
}

fn key(rng: &mut Rng, out: &mut String, name: &str) {
    out.push('"');
    if rng.chance(3) {
        let mut chars = name.chars();
        if let Some(first) = chars.next() {
            out.push_str(&format!("\\u{:04x}", u32::from(first)));
        }
        out.push_str(chars.as_str());
    } else {
        out.push_str(name);
    }
    out.push('"');
    out.push_str(rng.pick(&[":", ":", ":", " : ", ": "]));
}

fn scalar(rng: &mut Rng, out: &mut String, good: &str) {
    if !rng.chance(rng.noise) {
        out.push_str(good);
        return;
    }
    match rng.below(6) {
        0 => out.push_str("null"),
        1 => out.push('7'),
        2 => out.push_str(r#""""#),
        3 => text(rng, out, 3),
        4 => out.push_str("[]"),
        _ => out.push_str(good),
    }
}

fn members(rng: &mut Rng, out: &mut String, members: &mut Vec<(&str, String)>) {
    let len = members.len();
    for index in 0..len {
        let swap = rng.below(len);
        members.swap(index, swap);
    }
    if rng.chance(4) && !members.is_empty() {
        let duplicate = members[rng.below(members.len())].clone();
        members.push(duplicate);
    }
    out.push('{');
    for (index, (name, value)) in members.iter().enumerate() {
        if index > 0 {
            out.push_str(rng.pick(&[",", ",", ", "]));
        }
        key(rng, out, name);
        out.push_str(value);
    }
    out.push('}');
}

fn value(rng: &mut Rng, good: &str) -> String {
    let mut out = String::new();
    scalar(rng, &mut out, good);
    out
}

fn usage(rng: &mut Rng) -> String {
    let mut fields = vec![
        ("input_tokens", value(rng, "12")),
        ("output_tokens", value(rng, "340")),
        (
            "cache_read_input_tokens",
            value(rng, "18446744073709551615"),
        ),
        ("cache_creation_input_tokens", value(rng, "5")),
        ("speed", value(rng, r#""fast""#)),
        (
            "cache_creation",
            value(
                rng,
                r#"{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":4}"#,
            ),
        ),
        (
            "iterations",
            value(
                rng,
                r#"[{"type":"message","cache_creation":{"ephemeral_1h_input_tokens":5}},{"type":"advisor_message","model":"m"}]"#,
            ),
        ),
        ("service_tier", value(rng, r#""standard""#)),
    ];
    fields.retain(|_| !rng.chance(8));
    let mut out = String::new();
    members(rng, &mut out, &mut fields);
    out
}

fn content(rng: &mut Rng) -> String {
    let mut out = String::from("[");
    for index in 0..rng.below(4) {
        if index > 0 {
            out.push(',');
        }
        let mut block = vec![
            ("type", value(rng, r#""text""#)),
            ("text", {
                let mut text_value = String::new();
                text(rng, &mut text_value, 40);
                text_value
            }),
        ];
        if rng.chance(30) {
            block.push(("id", value(rng, r#""toolu_1""#)));
            block.push((
                "message",
                value(rng, r#"{"id":"nested","usage":{"input_tokens":1}}"#),
            ));
            block.push(("usage", usage(rng)));
        }
        members(rng, &mut out, &mut block);
    }
    out.push(']');
    out
}

fn message(rng: &mut Rng) -> String {
    if rng.chance(3) {
        return value(rng, "null");
    }
    let mut fields = vec![
        ("model", value(rng, r#""claude-sonnet-4-5""#)),
        ("id", value(rng, r#""msg_01""#)),
        ("type", value(rng, r#""message""#)),
        ("role", value(rng, r#""assistant""#)),
        ("content", content(rng)),
        (
            "usage",
            if rng.chance(95) {
                usage(rng)
            } else {
                value(rng, "null")
            },
        ),
        ("stop_reason", value(rng, "null")),
    ];
    fields.retain(|(name, _)| *name == "usage" || !rng.chance(10));
    let mut out = String::new();
    members(rng, &mut out, &mut fields);
    out
}

fn common(rng: &mut Rng) -> Vec<(&'static str, String)> {
    vec![
        ("timestamp", value(rng, r#""2026-11-20T10:00:00.000Z""#)),
        ("costUSD", value(rng, "0.25")),
        ("requestId", value(rng, r#""req_01""#)),
        ("isSidechain", value(rng, "false")),
        ("isApiErrorMessage", value(rng, "false")),
    ]
}

fn line(rng: &mut Rng) -> String {
    let mut fields = vec![
        ("parentUuid", value(rng, r#""p""#)),
        ("uuid", value(rng, r#""u""#)),
        ("type", value(rng, r#""assistant""#)),
        ("sessionId", value(rng, r#""s""#)),
        ("version", value(rng, r#""2.1.3""#)),
        ("cwd", value(rng, r#""C:\\Users\\me""#)),
    ];
    if rng.chance(25) {
        let mut inner = common(rng);
        inner.push(("message", message(rng)));
        inner.retain(|(name, _)| *name == "message" || !rng.chance(15));
        let mut progress = String::new();
        members(rng, &mut progress, &mut inner);
        let mut data = vec![
            ("message", progress),
            ("type", value(rng, r#""agent_progress""#)),
        ];
        let mut data_text = String::new();
        members(rng, &mut data_text, &mut data);
        fields.push(("data", data_text));
    }
    if rng.chance(85) {
        fields.extend(common(rng));
        fields.push(("message", message(rng)));
    }
    fields.retain(|(name, _)| matches!(*name, "message" | "data") || !rng.chance(10));
    let mut out = String::new();
    out.push_str(rng.pick(&["", "", " "]));
    members(rng, &mut out, &mut fields);
    out.push_str(rng.pick(&["", "", "", "\r", " "]));
    out
}

fn corrupt(rng: &mut Rng, line: &mut Vec<u8>) {
    for _ in 0..rng.below(3) {
        let at = rng.below(line.len() + 1);
        match rng.below(6) {
            0 => line.insert(at, [0xff, 0x80, 0xc3, b'\t', 0x01][rng.below(5)]),
            1 => line.truncate(at),
            2 if at < line.len() => {
                line.remove(at);
            }
            3 => line.insert(at, b"\",:{}[]\\ "[rng.below(9)]),
            _ => {}
        }
    }
}

#[test]
fn picked_usage_lines_match_the_full_parse() {
    let cases = 100_000;
    let mut rng = Rng {
        state: 0x9e37_79b9_7f4a_7c15,
        noise: 0,
    };
    let (mut decoded, mut picked) = (0, 0);
    for _ in 0..cases {
        rng.noise = [0, 0, 2, 5, 20][rng.below(5)];
        let mut bytes = line(&mut rng).into_bytes();
        if rng.chance(20) {
            corrupt(&mut rng, &mut bytes);
        }
        let text = JsonText::new(&bytes);
        let expected = format!("{:?}", parse_full_usage_line(&text));
        let actual = format!("{:?}", parse_usage_line(&text));
        assert_eq!(expected, actual, "{}", bytes.escape_ascii());
        if expected.starts_with("Ok") {
            decoded += 1;
            picked += usize::from(text.pick(USAGE_LINE).is_some());
        }
    }
    assert!(
        decoded * 5 > cases,
        "only {decoded} of {cases} lines decoded"
    );
    assert!(
        picked * 100 >= decoded * 95,
        "only {picked} of {decoded} decoded lines were picked"
    );
}
