use super::{Scan, Skim, Want, block_links, skim};

fn read(scan: Scan<Skim<'_>>) -> Skim<'_> {
    match scan {
        Scan::Read(skim) => skim,
        Scan::Skip | Scan::Unsure => panic!("the line was not read"),
    }
}

#[test]
fn reads_top_level_links_past_escaped_quotes_and_nested_values() {
    let line = br#"{"parentUuid":"p","nested":{"uuid":"inner","list":[1,{"a":"}"}]},"text":"say \"uuid\":\"x\" \\","type":"user","uuid":"u","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"\"]}"}]}}"#;

    let parsed = read(skim(line, Want::Link, |_, _| true));

    assert_eq!(parsed.parent, Some(&b"p"[..]));
    assert_eq!(parsed.uuid, Some(&b"u"[..]));
    assert_eq!(parsed.kind, Some(&b"user"[..]));
    let blocks = parsed.blocks.unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, Some(&b"tool_result"[..]));
    assert_eq!(blocks[0].tool_use_id, Some(&b"t"[..]));
}

#[test]
fn stops_at_an_untracked_parent() {
    let line = br#"{"sessionId":"s","parentUuid":"p","uuid":"u","truncated":"#;

    assert!(matches!(
        skim(line, Want::Link, |parent, _| parent != Some(b"p")),
        Scan::Skip
    ));
}

#[test]
fn rejects_a_line_without_a_parent() {
    let line = br#"{"parentUuid":null,"uuid":"u","type":"system"}"#;

    assert!(matches!(
        skim(line, Want::Link, |parent, _| parent.is_some()),
        Scan::Skip
    ));
}

#[test]
fn stops_reading_a_user_prompt_at_its_content() {
    let line = br#"{"parentUuid":"p","type":"user","uuid":"u","message":{"content":"prompt"#;

    assert!(matches!(skim(line, Want::Link, |_, _| true), Scan::Skip));
}

#[test]
fn reads_a_non_user_line_through_the_payload_after_its_links() {
    let line = br#"{"parentUuid":"p","sessionId":"s","type":"attachment","uuid":"u","attachment":{"a":[1,"}"]}}"#;

    let parsed = read(skim(line, Want::Link, |_, _| true));

    assert_eq!(parsed.uuid, Some(&b"u"[..]));
    assert!(parsed.blocks.is_none());
}

#[test]
fn leaves_a_line_continued_by_another_record_to_the_full_parse() {
    let line = br#"{"parentUuid":"p","sessionId":"s","type":"attachment","uuid":"u","times{"parentUuid":"p","type":"system","uuid":"v","sessionId":"s"}"#;

    assert!(matches!(skim(line, Want::Link, |_, _| true), Scan::Unsure));
}

#[test]
fn leaves_a_line_with_bytes_after_its_object_to_the_full_parse() {
    let line = br#"{"parentUuid":"p","sessionId":"s","type":"attachment","uuid":"u"}{"uuid":"v"}"#;

    assert!(matches!(skim(line, Want::Link, |_, _| true), Scan::Unsure));
}

#[test]
fn notices_usage_on_a_line_that_may_carry_it() {
    let line = br#"{"parentUuid":"p","type":"assistant","uuid":"u","message":{"usage":{"input_tokens":1}}}"#;

    assert!(read(skim(line, Want::Block, |_, _| true)).has_usage);
}

#[test]
fn reads_the_record_type_that_follows_the_message() {
    let line = br#"{"parentUuid":"p","uuid":"u","message":{"content":"prompt"},"type":"user"}"#;

    let parsed = read(skim(line, Want::Link, |_, _| true));

    assert_eq!(parsed.kind, Some(&b"user"[..]));
}

#[test]
fn reads_the_top_level_uuid_after_a_message_whose_tool_input_holds_one() {
    let line = br#"{"parentUuid":"p","message":{"content":[{"type":"tool_use","id":"t","input":{"uuid":"nested"}}],"usage":{"input_tokens":1}},"type":"assistant","uuid":"u","timestamp":"x"}"#;

    let links = block_links(line).unwrap();

    assert_eq!(links.uuid, Some(&b"u"[..]));
    assert_eq!(links.parent, Some(&b"p"[..]));
}

#[test]
fn reads_the_top_level_uuid_past_a_trailing_object_that_holds_one() {
    let line = br#"{"parentUuid":"p","message":{"content":[]},"toolUseResult":{"a":1,"uuid":"nested"},"uuid":"u"}"#;

    assert_eq!(block_links(line).unwrap().uuid, Some(&b"u"[..]));
}

#[test]
fn counts_only_tool_use_blocks_of_the_message_content() {
    let line = br#"{"parentUuid":"p","uuid":"u","message":{"content":[{"type":"text","text":"say \"type\":\"tool_use\""},{"type":"tool_use","id":"own","input":{"type":"tool_use","id":"inner"}}]}}"#;

    let links = block_links(line).unwrap();

    assert_eq!(links.tool_uses.as_slice(), [&b"own"[..]]);
}

#[test]
fn stops_reading_content_once_no_tool_use_remains() {
    let line = br#"{"parentUuid":"p","uuid":"u","message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"#;

    assert!(block_links(line).unwrap().tool_uses.is_empty());
}

#[test]
fn leaves_a_line_cut_after_a_scalar_and_continued_to_the_full_parse() {
    let line = br#"{"type":"attachment","parentUuid":"p","uuid":"u","isSidechain":false{"parentUuid":null,"sessionId":"s","type":"system","uuid":"v"}"#;

    assert!(matches!(skim(line, Want::Link, |_, _| true), Scan::Unsure));
}
