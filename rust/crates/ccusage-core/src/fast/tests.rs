use super::suffix_string;

#[test]
fn suffix_string_builds_without_formatting() {
    assert_eq!(
        suffix_string("claude-sonnet-4", "-fast"),
        "claude-sonnet-4-fast"
    );
}
