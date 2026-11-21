use super::LinePrefilter;

#[test]
fn line_prefilter_all_requires_every_marker() {
    let prefilter = LinePrefilter::all(&[b"\"usage\"", b"\"message\""]);

    assert!(prefilter.matches(br#"{"message":{"usage":{"input":1}}}"#));
    assert!(!prefilter.matches(br#"{"message":{"role":"user"}}"#));
    assert!(!prefilter.matches(br#"{"usage":{"input":1}}"#));
}

#[test]
fn line_prefilter_any_requires_one_marker() {
    let prefilter = LinePrefilter::any(&[b"\"model_change\"", b"\"usage\""]);

    assert!(prefilter.matches(br#"{"type":"model_change"}"#));
    assert!(prefilter.matches(br#"{"message":{"usage":{}}}"#));
    assert!(!prefilter.matches(br#"{"type":"message"}"#));
}

#[test]
fn line_prefilter_admits_markers_spelled_with_json_escapes() {
    let bare = LinePrefilter::any(&[b"assistant/message"]);
    let quoted = LinePrefilter::all(&[br#""usage":{"#]);

    assert!(bare.matches(br#"{"type":"assistant\/message"}"#));
    assert!(bare.matches(br#"{"type":"\u0061ssistant\u002fmessage"}"#));
    assert!(bare.matches(br#"{"type":"assistant/messag\u0065"}"#));
    assert!(quoted.matches(br#"{"message":{"us\u0061ge":{"input":1}}}"#));
    assert!(quoted.matches(br#"{"message":{"\u0075sage":{}}}"#));
}

#[test]
fn line_prefilter_rejects_escapes_that_do_not_spell_a_marker() {
    let bare = LinePrefilter::any(&[b"assistant/message"]);
    let quoted = LinePrefilter::all(&[br#""usage":{"#]);

    assert!(!bare.matches(br#"{"type":"tool","text":"a\\\/b \u001b[1m\n\"x\""}"#));
    assert!(!bare.matches(br#"{"type":"assistant\/messenger","t":"assistant\\/message"}"#));
    assert!(!bare.matches(br#"{"type":"assistant\u00e9message"}"#));
    assert!(!quoted.matches(br#"{"text":"\"usage\":{"}"#));
    assert!(!quoted.matches(br#"{"message":{"us\u0061ge":1}}"#));
    assert!(!quoted.matches(br#"{"message":{"us\u0061g":{}}}"#));
}

#[test]
fn line_prefilter_all_admits_each_marker_verbatim_or_escaped() {
    let prefilter = LinePrefilter::all(&[br#""a/b""#, br#""c/d""#, br#""e""#]);

    assert!(prefilter.matches(br#"{"a\/b":1,"c\/d":2,"e":3}"#));
    assert!(!prefilter.matches(br#"{"a\/b":1,"e":3}"#));
}

#[test]
fn line_prefilter_reads_a_unicode_escape_only_after_paired_backslashes() {
    let prefilter = LinePrefilter::all(&[br#""usage":{"#]);

    assert!(prefilter.matches(br#"{"n":"\\","us\u0061ge":{}}"#));
    assert!(prefilter.matches(br#"{"n":"\\\\","us\u0061ge":{}}"#));
    assert!(!prefilter.matches(br#"{"us\\u0061ge":{}}"#));
    assert!(!prefilter.matches(br#"{"n":"\n\"\t","x":{}}"#));
}

#[test]
fn line_prefilter_follows_short_escapes_when_a_marker_holds_a_slash() {
    let prefilter = LinePrefilter::any(&[b"a/b"]);

    assert!(prefilter.matches(br#"{"t":"\n a\/b"}"#));
    assert!(prefilter.matches(br#"{"t":"a\u002fb"}"#));
    assert!(!prefilter.matches(br#"{"t":"a\\/b"}"#));
}

#[test]
fn line_prefilter_matches_escaped_ignores_verbatim_markers() {
    let prefilter = LinePrefilter::any(&[b"a/b", b"c/d"]);

    assert!(prefilter.matches_escaped(br#"{"t":"a/b","u":"c\/d"}"#));
    assert!(!prefilter.matches_escaped(br#"{"t":"a/b","n":"\n"}"#));
}

fn unicode_escape(hex: &str) -> String {
    ["\\", "u", hex].concat()
}

#[test]
fn line_prefilter_finds_bracketed_markers_through_their_bracket() {
    let prefilter = LinePrefilter::all(&[br#""usage":{"#]);
    let u = unicode_escape("0075");
    let a = unicode_escape("0061");
    let e = unicode_escape("0065");

    for line in [
        r#"{"message":{"usage":{}}}"#.to_owned(),
        format!(r#"{{"message":{{"{u}sage":{{}}}}}}"#),
        format!(r#"{{"message":{{"us{a}g{e}":{{}}}}}}"#),
        format!(r#"{{"n":"\\","us{a}ge":{{}}}}"#),
    ] {
        assert!(prefilter.matches(line.as_bytes()), "{line}");
    }
    for line in [
        r#"{"text":"\"usage\":{"}"#.to_owned(),
        format!(r#"{{"us\\{a}ge":{{}}}}"#),
        format!(r#"{{"us{a}ge":[]}}"#),
        format!(r#"{{us{a}ge":{{}}}}"#),
        format!(r#"{{"{}":{{}}}}"#, unicode_escape("00e9")),
    ] {
        assert!(!prefilter.matches(line.as_bytes()), "{line}");
    }
}

#[test]
fn line_prefilter_admits_json_whitespace_before_a_bracket() {
    let prefilter = LinePrefilter::all(&[br#""usage":{"#]);
    let u = unicode_escape("0075");

    for line in [
        r#"{"message":{"usage":{}}}"#.to_owned(),
        r#"{"message":{"usage" : {}}}"#.to_owned(),
        r#"{"message":{"usage": {}}}"#.to_owned(),
        r#"{"message":{"usage" :{}}}"#.to_owned(),
        "{\"message\":{\"usage\"\t:\t{}}}".to_owned(),
        "{\"message\":{\"usage\"\r\n:\n {}}}".to_owned(),
        format!(r#"{{"message":{{"{u}sage" : {{}}}}}}"#),
        format!("{{\"n\":\"\\n\",\"{u}sage\":\t{{}}}}"),
    ] {
        assert!(prefilter.matches(line.as_bytes()), "{line:?}");
    }
    for line in [
        r#"{"message":{"usage ":{}}}"#.to_owned(),
        r#"{"message":{"usage": []}}"#.to_owned(),
        r#"{"message":{"usage" x {}}}"#.to_owned(),
        r#"{"message":{"usage" : 1, "x": {}}}"#.to_owned(),
        "{\"message\":{\"usage\"\u{a0}:{}}}".to_owned(),
        format!(r#"{{"message":{{"{u}sage" : 1}}}}"#),
    ] {
        assert!(!prefilter.matches(line.as_bytes()), "{line:?}");
    }
}

#[test]
fn line_prefilter_follows_escapes_after_a_bracket() {
    let prefilter = LinePrefilter::any(&[br#""k":{"a/b"#]);

    assert!(prefilter.matches(br#"{"k":{"a\/b":1}}"#));
    assert!(
        prefilter.matches(format!(r#"{{"k":{{"{}/b":1}}}}"#, unicode_escape("0061")).as_bytes())
    );
    assert!(!prefilter.matches(br#"{"k":{"a\\/b":1}}"#));
    assert!(!prefilter.matches(br#"{"k":{ "a/b":1}}"#));
}

fn spellings(marker: &[u8], line: &[u8], start: usize, whitespace: bool) -> Vec<usize> {
    let mut in_string = marker.first() != Some(&b'"');
    let content = marker
        .iter()
        .map(|&byte| {
            in_string ^= byte == b'"';
            byte != b'"' && in_string
        })
        .collect::<Vec<_>>();
    let anchor =
        (0..marker.len()).find(|&index| !content[index] && b"{}[]".contains(&marker[index]));
    let spaced = |index: usize| {
        anchor.is_some_and(|anchor| {
            whitespace
                && index > 0
                && index <= anchor
                && !content[index - 1]
                && (index..=anchor).all(|run| !content[run])
                && [marker[index - 1], marker[index]]
                    .iter()
                    .any(|byte| b":,{}[]".contains(byte))
        })
    };
    let mut found = Vec::new();
    let mut pending = vec![(0, start, 0)];
    while let Some((index, at, escapes)) = pending.pop() {
        if index == marker.len() {
            found.push(escapes);
            continue;
        }
        if line.get(at) == Some(&marker[index]) {
            pending.push((index + 1, at + 1, escapes));
        }
        if spaced(index) && line.get(at).is_some_and(|byte| b" \t\n\r".contains(byte)) {
            pending.push((index, at + 1, escapes));
        }
        let real_escape = index > 0
            || line[..at]
                .iter()
                .rev()
                .take_while(|&&byte| byte == b'\\')
                .count()
                % 2
                == 0;
        if !content[index] || line.get(at) != Some(&b'\\') || !real_escape {
            continue;
        }
        let short = match line.get(at + 1) {
            Some(b'"') => Some(b'"'),
            Some(b'\\') => Some(b'\\'),
            Some(b'/') => Some(b'/'),
            Some(b'b') => Some(0x08),
            Some(b'f') => Some(0x0c),
            Some(b'n') => Some(b'\n'),
            Some(b'r') => Some(b'\r'),
            Some(b't') => Some(b'\t'),
            _ => None,
        };
        if short == Some(marker[index]) {
            pending.push((index + 1, at + 2, escapes + 1));
        }
        let unicode = line
            .get(at + 1..at + 6)
            .filter(|escape| escape[0] == b'u')
            .and_then(|escape| std::str::from_utf8(&escape[1..]).ok())
            .filter(|hex| hex.bytes().all(|digit| digit.is_ascii_hexdigit()))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok());
        if unicode == Some(u32::from(marker[index])) {
            pending.push((index + 1, at + 6, escapes + 1));
        }
    }
    found
}

fn spells(marker: &[u8], line: &[u8], escaped_only: bool) -> bool {
    (0..=line.len()).any(|start| {
        spellings(marker, line, start, !escaped_only)
            .into_iter()
            .any(|escapes| !escaped_only || escapes > 0)
    })
}

#[test]
fn line_prefilter_admits_exactly_the_lines_that_spell_its_markers() {
    let mut fragments = [
        &b"\""[..],
        b":",
        b"{",
        b"}",
        b"[",
        b"]",
        b",",
        b" ",
        b"\t",
        b"\\",
        b"\\\\",
        br"\/",
        b"/",
        br"\n",
        br#"\""#,
        b"u",
        b"s",
        b"a",
        b"g",
        b"e",
        b"usage",
        b"type",
        b"x",
        b"_",
    ]
    .map(<[u8]>::to_vec)
    .to_vec();
    for hex in [
        "0075", "0061", "0065", "0045", "005f", "005F", "002f", "0078", "00e9", "00", "006",
    ] {
        fragments.push(unicode_escape(hex).into_bytes());
    }
    let marker_sets: [&[&[u8]]; 10] = [
        &[br#""usage":{"#],
        &[br#""usage":{"#, br#""type""#],
        &[br#""x":{"a/e"#, b"us_e"],
        &[br#""a":[]"#],
        &[br#"]"x/a""#, b"a/e"],
        &[br#"{"a":["#],
        &[br#"{"#, br#""a_x""#],
        &[br#"a":{"#],
        &[br#"a_x":{"#],
        &[br#"a/e":["#, br#""type""#],
    ];
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % bound as u64).unwrap_or_default()
    };
    for markers in marker_sets {
        let all = LinePrefilter::all(markers);
        let any = LinePrefilter::any(markers);
        let old_all = super::reference::LinePrefilter::all(markers);
        let old_any = super::reference::LinePrefilter::any(markers);
        for _ in 0..5_000 {
            let line = (0..next(16))
                .flat_map(|_| fragments[next(fragments.len())].clone())
                .collect::<Vec<_>>();
            let text = String::from_utf8_lossy(&line);
            if !line.iter().any(|byte| b" \t".contains(byte)) {
                assert_eq!(all.matches(&line), old_all.matches(&line), "old all {text}");
                assert_eq!(any.matches(&line), old_any.matches(&line), "old any {text}");
            }
            assert_eq!(
                any.matches_escaped(&line),
                old_any.matches_escaped(&line),
                "old escaped {text}"
            );
            let spelled = markers.iter().map(|marker| spells(marker, &line, false));
            assert_eq!(all.matches(&line), spelled.clone().all(|s| s), "all {text}");
            assert_eq!(any.matches(&line), spelled.clone().any(|s| s), "any {text}");
            assert_eq!(
                any.matches_escaped(&line),
                markers.iter().any(|marker| spells(marker, &line, true)),
                "escaped {text}"
            );
        }
    }
}

#[test]
fn line_prefilter_matches_the_previous_implementation_on_mutated_markers() {
    let marker_sets: &[&[&[u8]]] = &[
        &[],
        &[b""],
        &[br#""usage":{"#],
        &[br#""usage":{"#, br#""type""#],
        &[br#""x":{"a/e"#, b"us_e"],
        &[br#""a":[]"#],
        &[br#"]"x/a""#, b"a/e"],
        &[br#"{"a":["#],
        &[br#"{"#, br#""a_x""#],
        &[br#"a_x":{"#],
        &[br#"a/e":["#, br#""type""#],
    ];
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let mut next = |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % bound as u64).unwrap_or_default()
    };
    for &markers in marker_sets {
        let all = LinePrefilter::all(markers);
        let any = LinePrefilter::any(markers);
        let old_all = super::reference::LinePrefilter::all(markers);
        let old_any = super::reference::LinePrefilter::any(markers);
        for _ in 0..5_000 {
            let mut line = br#"{"decoy":"\\u0061 \u00e9 \"usage\":{","value":"#.to_vec();
            line.extend(std::iter::repeat_n(b'\\', next(8)));
            for &marker in markers {
                let mut in_string = marker.first() != Some(&b'"');
                for &byte in marker {
                    in_string ^= byte == b'"';
                    match (byte != b'"' && in_string, next(8)) {
                        (true, 0) => line.extend(unicode_escape(&format!("{byte:04x}")).bytes()),
                        (true, 1) => line.extend(unicode_escape(&format!("{byte:04X}")).bytes()),
                        (true, 2) => {
                            line.extend(unicode_escape(&format!("{byte:04x}")[..3]).bytes())
                        }
                        (true, 3) => {
                            line.extend_from_slice(b"\\\\");
                            line.extend(unicode_escape(&format!("{byte:04x}")).bytes());
                        }
                        (_, 4) => line.push(b'x'),
                        (true, 5) if byte == b'/' => line.extend_from_slice(br"\/"),
                        _ => line.push(byte),
                    }
                }
                line.extend_from_slice(br#", "x":{},"#);
            }
            let text = String::from_utf8_lossy(&line);
            assert_eq!(all.matches(&line), old_all.matches(&line), "all {text}");
            assert_eq!(any.matches(&line), old_any.matches(&line), "any {text}");
            assert_eq!(
                any.matches_escaped(&line),
                old_any.matches_escaped(&line),
                "escaped {text}"
            );
        }
    }
}
