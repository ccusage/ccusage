#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use ./contribution-gate/coauthor.nu [coauthor-validation]
use ./contribution-gate/requests.nu [coauthor-email render-prompt]

def expect [name: string, actual, expected]: nothing -> nothing {
    if $actual != $expected {
        let expected_json = $expected | to json
        let actual_json = $actual | to json
        error make {msg: $"($name): expected ($expected_json), got ($actual_json)"}
    }
}

def test-coauthor-validation []: nothing -> nothing {
    let trailer = 'Co-authored-by: alice <alice@users.noreply.github.com>'
    let message = $"Implement the fix\n\n($trailer)"
    (expect
        'accepts exact resolved attribution'
        (coauthor-validation $message [42] $trailer 42)
        {trailer_ok: true, author_ok: true}
    )
    (expect
        'rejects a missing trailer'
        (coauthor-validation 'Implement the fix' [42] $trailer 42)
        {trailer_ok: false, author_ok: true}
    )
    expect 'rejects an additional co-author' (
        coauthor-validation $"($message)\nCo-authored-by: bob <bob@example.com>" [42 99] $trailer 42
    ) {trailer_ok: false, author_ok: true}
    (expect
        'rejects unresolved attribution'
        (coauthor-validation $message [] $trailer 42)
        {trailer_ok: true, author_ok: false}
    )
}

def test-coauthor-email []: nothing -> nothing {
    expect 'keeps a public account email' (
        coauthor-email alice 42 {email: 'alice@example.com' created_at: '2020-01-01T00:00:00Z'}
    ) 'alice@example.com'
    expect 'uses the legacy no-reply format' (
        coauthor-email alice 42 {email: null created_at: '2017-07-17T23:59:59Z'}
    ) 'alice@users.noreply.github.com'
    expect 'uses the current no-reply format' (
        coauthor-email alice 42 {email: null created_at: '2017-07-18T00:00:00Z'}
    ) '42+alice@users.noreply.github.com'
}

def test-prompt-rendering []: nothing -> nothing {
    let rendered = render-prompt 'issue-implementation.md' {
        ISSUE_NUMBER: '42'
        REPOSITORY: 'ccusage/ccusage'
        IMPLEMENTATION_MARKER: '<!-- marker -->'
        COAUTHOR_TRAILER: 'Co-authored-by: alice <alice@example.com>'
    }
    (expect
        'renders prompt values'
        ($rendered | str contains 'Issue number: #42 in ccusage/ccusage.')
        true
    )
    (expect
        'renders the implementation marker'
        ($rendered | str contains '<!-- marker -->')
        true
    )
    (expect
        'renders the co-author trailer'
        ($rendered | str contains 'Co-authored-by: alice <alice@example.com>')
        true
    )
    (expect
        'does not leave template placeholders'
        ($rendered | str contains '{{')
        false
    )
}

def main [] {
    test-coauthor-validation
    test-coauthor-email
    test-prompt-rendering
    print 'contribution-gate Nushell tests passed.'
}
