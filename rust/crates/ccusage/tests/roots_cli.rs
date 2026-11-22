#![cfg(test)]

use ccusage_test_support::Fixture;

mod support;

#[test]
fn debug_reports_missing_home_once_for_every_default_root_reader() {
    let fixture = Fixture::new();
    for agent in ["zcode", "goose", "antigravity"] {
        let output = support::hermetic_command(fixture.root())
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .env("LOG_LEVEL", "0")
            .args([agent, "daily", "--debug", "--offline", "--json"])
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            stderr
                .matches("Unable to resolve default data roots: home directory is not set")
                .count(),
            1,
            "{agent}: {stderr}"
        );
    }
}
