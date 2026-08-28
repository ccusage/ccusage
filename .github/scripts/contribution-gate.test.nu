#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use ./contribution-gate/coauthor.nu [coauthor-validation]
use ./contribution-gate/context.nu [issue-context-record]
use ./contribution-gate/core.nu [parse-issue-number]
use ./contribution-gate/mutations.nu [is-contribution-gate-comment]
use ./contribution-gate/verdict.nu [issue-verdict-record]
use ./contribution-gate/requests.nu [
    CLOSING_PULL_REQUEST_QUERY
    cleanup-unvalidated-publication
    cleanup-operation-errors
    closing-pull-request-nodes
    coauthor-email
    competing-closing-pull-request
    existing-implementation-pull-request
    implementation-branch
    implementation-pull-request-body
    implementation-pull-request-for-branch
    implementation-result
    implementation-title
    neutralize-closing-references
    open-closing-pull-request
    render-prompt
    retryable-gh-api-command-error
    retry-pull-request-lookup
    with-failure-cleanup
]

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
    let attribution = [
        {email: 'alice@users.noreply.github.com', user_id: 42}
    ]
    (expect
        'accepts exact resolved attribution'
        (coauthor-validation $message $attribution $trailer 'alice@users.noreply.github.com' 42)
        {trailer_ok: true, author_ok: true}
    )
    (expect
        'rejects a missing trailer'
        (coauthor-validation 'Implement the fix' $attribution $trailer 'alice@users.noreply.github.com' 42)
        {trailer_ok: false, author_ok: true}
    )
    expect 'rejects an additional co-author' (
        coauthor-validation $"($message)\nCo-authored-by: bob <bob@example.com>" $attribution $trailer 'alice@users.noreply.github.com' 42
    ) {trailer_ok: false, author_ok: true}
    (expect
        'rejects unresolved attribution'
        (coauthor-validation $message [] $trailer 'alice@users.noreply.github.com' 42)
        {trailer_ok: true, author_ok: false}
    )
    (expect
        'rejects an issue author resolved outside the expected trailer'
        (coauthor-validation
            $message
            [{email: 'bot@example.com', user_id: 42}, {email: 'alice@users.noreply.github.com', user_id: 99}]
            $trailer
            'alice@users.noreply.github.com'
            42
        )
        {trailer_ok: true, author_ok: false}
    )
}

def test-coauthor-email []: nothing -> nothing {
    expect 'keeps a public account email' (
        coauthor-email alice 42 {email: 'alice@example.com' created_at: '2020-01-01T00:00:00Z'}
    ) 'alice@example.com'
    let legacy_result = (try {
        coauthor-email alice 42 {email: null created_at: '2017-07-17T23:59:59Z'}
    } catch {
        null
    })
    expect 'rejects an ambiguous legacy no-reply format' $legacy_result null
    expect 'uses the current no-reply format' (
        coauthor-email alice 42 {email: null created_at: '2017-07-18T00:00:00Z'}
    ) '42+alice@users.noreply.github.com'
}

def test-prompt-rendering []: nothing -> nothing {
    let rendered = render-prompt 'issue-implementation.md' {
        ISSUE_NUMBER: '42'
        REPOSITORY: 'ccusage/ccusage'
    }
    (expect
        'renders prompt values'
        ($rendered | str contains 'Issue number: #42 in ccusage/ccusage.')
        true
    )
    (expect
        'leaves implementation changes uncommitted'
        ($rendered | str contains 'Do not commit, push, or create a pull request.')
        true
    )
    (expect
        'requests PR metadata with a prepared result'
        ($rendered | str contains '"implementation":"prepared"')
        true
    )
    (expect
        'states the PR metadata validation contract'
        [
            ($rendered | str contains 'The title must be exactly one line.')
            ($rendered | str contains 'The body must be non-empty.')
        ]
        [true true]
    )
    (expect
        'does not leave template placeholders'
        ($rendered | str contains '{{')
        false
    )
}

def test-existing-implementation-pull-request []: nothing -> nothing {
    let matching = {
        number: 100
        body: '<!-- pullfrog-accepted-issue: #42 request-abc -->'
        user: {login: 'github-actions[bot]'}
        head: {
            ref: 'pullfrog/issue-42-run-123'
            repo: {full_name: 'ccusage/ccusage'}
        }
    }
    let pull_requests = [
        $matching
        {
            number: 101
            body: '<!-- pullfrog-accepted-issue: #42 request-forged -->'
            user: {login: alice}
            head: {
                ref: 'feature/copied-marker'
                repo: {full_name: 'alice/ccusage'}
            }
        }
        {
            number: 102
            body: null
            user: {login: 'github-actions[bot]'}
            head: {
                ref: 'pullfrog/issue-420-run-456'
                repo: {full_name: 'ccusage/ccusage'}
            }
        }
    ]

    expect 'finds an existing implementation PR for the same issue' (
        existing-implementation-pull-request $pull_requests 'ccusage/ccusage' 42
    ) $matching
    expect 'does not trust a copied marker in an external PR' (
        existing-implementation-pull-request ($pull_requests | skip 1 | take 1) 'ccusage/ccusage' 42
    ) null
    expect 'does not match another issue branch' (
        existing-implementation-pull-request $pull_requests 'ccusage/ccusage' 7
    ) null
    expect 'finds the run-owned pull request by its exact branch' (
        implementation-pull-request-for-branch
            $pull_requests
            'ccusage/ccusage'
            'pullfrog/issue-42-run-123'
    ) $matching
    expect 'does not use another run-owned branch as a publication fallback' (
        implementation-pull-request-for-branch
            $pull_requests
            'ccusage/ccusage'
            'pullfrog/issue-42-run-999'
    ) null

    let closing_pull_request = {number: 1209, state: OPEN, url: 'https://github.com/ccusage/ccusage/pull/1209'}
    expect 'finds an open pull request that GitHub recognizes as closing the issue' (
        open-closing-pull-request [$closing_pull_request]
    ) {number: 1209, html_url: 'https://github.com/ccusage/ccusage/pull/1209'}
    expect 'ignores a closed pull request' (
        open-closing-pull-request [($closing_pull_request | update state CLOSED)]
    ) null
    expect 'queries GitHub normalized closing relationships' (
        $CLOSING_PULL_REQUEST_QUERY | str contains 'closedByPullRequestsReferences'
    ) true
    expect 'requests closing-relationship pagination state' (
        $CLOSING_PULL_REQUEST_QUERY | str contains 'pageInfo { hasNextPage }'
    ) true
    expect 'does not query generic timeline cross-references' (
        $CLOSING_PULL_REQUEST_QUERY | str contains 'timeline'
    ) false
    expect 'returns null when GitHub reports no closing pull request' (
        open-closing-pull-request []
    ) null
    expect 'ignores a mention-only timeline record outside the GraphQL relationship' (
        open-closing-pull-request [{event: cross-referenced, source: {type: issue}}]
    ) null
    expect 'ignores malformed closing pull request data' (
        open-closing-pull-request [{number: 0, state: OPEN, url: ''}]
    ) null

    let connection = {
        nodes: [$closing_pull_request]
        pageInfo: {hasNextPage: false}
    }
    expect 'accepts a complete closing pull request page' (
        closing-pull-request-nodes $connection
    ) [$closing_pull_request]
    let incomplete_page = try {
        closing-pull-request-nodes ($connection | update pageInfo.hasNextPage true)
        'accepted'
    } catch {
        'rejected'
    }
    (expect
        'fails closed when closing pull requests exceed one page'
        $incomplete_page
        rejected
    )

    let own_pull_request = $closing_pull_request | update number 1300
    expect 'finds another closing pull request after publication' (
        competing-closing-pull-request [$own_pull_request $closing_pull_request] 1300
    ) {number: 1209, html_url: 'https://github.com/ccusage/ccusage/pull/1209'}
    expect 'does not treat the newly created pull request as its own competitor' (
        competing-closing-pull-request [$own_pull_request] 1300
    ) null
}

def test-implementation-result []: nothing -> nothing {
    expect 'accepts prepared PR metadata' (
        implementation-result '{"implementation":"prepared","title":"fix: count cache writes","body":"Fixes the missing cost.\n\nTests: focused adapter test."}'
    ) {implementation: prepared, title: 'fix: count cache writes', body: "Fixes the missing cost.\n\nTests: focused adapter test."}
    expect 'accepts a declined implementation' (
        implementation-result '{"implementation":"none","title":"","body":""}'
    ) {implementation: none, title: '', body: ''}

    let invalid_results = [
        '{"implementation":"prepared","title":"","body":"body"}'
        '{"implementation":"prepared","title":"title\nsecond line","body":"body"}'
        '{"implementation":"prepared","title":"title","body":""}'
        '{"implementation":"created","title":"title","body":"body"}'
        'not json'
    ]
    $invalid_results | each {|result|
        let outcome = (try {
            implementation-result $result
            'accepted'
        } catch {
            'rejected'
        })
        expect $"rejects invalid implementation result ($result)" $outcome 'rejected'
    } | ignore
}

def test-implementation-publication []: nothing -> nothing {
    expect 'uses a run-owned implementation branch' (
        implementation-branch 42 123 2
    ) 'pullfrog/issue-42-run-123-attempt-2'

    let invalid_run_values = [
        {run_id: 0, run_attempt: 1}
        {run_id: 123, run_attempt: 0}
    ]
    $invalid_run_values | each {|value|
        let outcome = (try {
            implementation-branch 42 $value.run_id $value.run_attempt
            'accepted'
        } catch {
            'rejected'
        })
        expect 'rejects an invalid workflow run identity' $outcome 'rejected'
    } | ignore

    expect 'adds workflow-owned PR metadata' (
        implementation-pull-request-body '<!-- marker -->' "Fixes #7.\n\nImplemented the fix.\n\nTests: focused test." 42
    ) "<!-- marker -->\n\nReferences #7.\n\nImplemented the fix.\n\nTests: focused test.\n\nCloses #42"

    expect 'neutralizes model-provided closing references only' (
        neutralize-closing-references "Fixes #7. resolves ccusage/ccusage#8. CLOSES https://github.com/ccusage/ccusage/issues/9. Fixed the parser."
    ) "References #7. References ccusage/ccusage#8. References https://github.com/ccusage/ccusage/issues/9. Fixed the parser."

    expect 'neutralizes closing references in implementation titles' (
        implementation-title 'fix: resolves #999 without closing the reported issue'
    ) 'fix: References #999 without closing the reported issue'

    let maximum_raw_title = $"Fixes #9(1..232 | each { 'x' } | str join)"
    expect 'rejects a sanitized implementation title over 240 characters' (try {
        implementation-title $maximum_raw_title
        'accepted'
    } catch {
        'rejected'
    }) 'rejected'

    expect 'attempts every discard operation before reporting failures' (
        cleanup-operation-errors
            { error make {msg: 'close failed'} }
            { error make {msg: 'branch delete failed'} }
    ) ['close pull request: close failed' 'delete branch: branch delete failed']
    expect 'reports only a pull request close failure' (
        cleanup-operation-errors
            { error make {msg: 'close failed'} }
            { null }
    ) ['close pull request: close failed']
    expect 'reports only a branch deletion failure' (
        cleanup-operation-errors
            { null }
            { error make {msg: 'branch delete failed'} }
    ) ['delete branch: branch delete failed']

    expect 'does not clean up a successful publication operation' (
        with-failure-cleanup
            { 'published' }
            { error make {msg: 'cleanup must not run'} }
    ) published
    expect 'preserves a publication failure after successful cleanup' (try {
        (with-failure-cleanup
            { error make {msg: 'publication failed'} }
            { null }
        )
        'accepted'
    } catch {|error|
        $error.msg
    }) 'publication failed'
    expect 'reports publication and cleanup failures together' (try {
        (with-failure-cleanup
            { error make {msg: 'publication failed'} }
            { error make {msg: 'branch cleanup failed'} }
        )
        'accepted'
    } catch {|error|
        $error.msg
    }) 'publication failed; cleanup also failed: branch cleanup failed'

    expect 'recovers and closes an unvalidated publication before deleting its branch' (
        cleanup-unvalidated-publication
            { {number: 77} }
            {|number| error make {msg: $"close ($number) failed"} }
            { error make {msg: 'branch delete failed'} }
    ) [
        'close pull request: close 77 failed'
        'delete branch: branch delete failed'
    ]
    expect 'skips pull request closure when publication did not create one' (
        cleanup-unvalidated-publication
            { null }
            {|_| error make {msg: 'close must not run'} }
            { null }
    ) []
    expect 'retries an eventually consistent pull request lookup' (
        retry-pull-request-lookup
            {|attempt| if $attempt == 3 { {number: 77} } else { null } }
            {|_| null }
            3
    ) {number: 77}
    expect 'returns null after exhausting pull request lookup retries' (
        retry-pull-request-lookup
            {|_| null }
            {|_| null }
            3
    ) null
    let transient_lookup_errors = [
        'Transient GitHub pull request lookup: gh api failed: HTTP 503'
        'Transient GitHub pull request lookup: dial tcp: lookup api.github.com: no such host'
        'Transient GitHub pull request lookup: proxyconnect tcp: connection refused'
        'Transient GitHub pull request lookup: network is unreachable'
        'Transient GitHub pull request lookup: You have triggered an abuse detection mechanism (HTTP 403)'
    ]
    $transient_lookup_errors | each {|message|
        expect 'retries a transient GitHub lookup failure' (
            retry-pull-request-lookup
                {|attempt|
                    if $attempt == 1 {
                        error make {msg: $message}
                    }
                    {number: 77}
                }
                {|_| null }
                2
        ) {number: 77}
    } | ignore
    expect 'fails closed without retrying an authentication error' (try {
        (retry-pull-request-lookup
            {|_| error make {msg: 'gh api failed: HTTP 401'} }
            {|_| error make {msg: 'wait must not run'} }
            3
        )
        'accepted'
    } catch {|error|
        $error.msg
    }) 'gh api failed: HTTP 401'
    expect 'fails closed after exhausting transient lookup retries' (try {
        (retry-pull-request-lookup
            {|_| error make {msg: 'Transient GitHub pull request lookup: API rate limit exceeded (HTTP 403)'} }
            {|_| null }
            2
        )
        'accepted'
    } catch {|error|
        $error.msg
    }) 'Pull request lookup still failed after retrying: Transient GitHub pull request lookup: API rate limit exceeded (HTTP 403)'
    expect 'does not retry malformed JSON containing a retry keyword' (try {
        (retry-pull-request-lookup
            {|_| error make {msg: 'gh api returned invalid JSON: {"message":"rate limit"}'} }
            {|_| error make {msg: 'wait must not run'} }
            3
        )
        'accepted'
    } catch {|error|
        $error.msg
    }) 'gh api returned invalid JSON: {"message":"rate limit"}'

    let retryable_command_errors = [
        'gh: upstream failed (HTTP 503)'
        'gh: API rate limit exceeded (HTTP 403)'
        'dial tcp: lookup api.github.com: no such host'
        'proxyconnect tcp: connection refused'
        'network is unreachable'
    ]
    $retryable_command_errors | each {|message|
        expect 'classifies a trusted command failure as retryable' (
            retryable-gh-api-command-error $message
        ) true
    } | ignore
    expect 'does not classify an ordinary permission response as retryable' (
        retryable-gh-api-command-error 'gh: Resource not accessible by integration (HTTP 403)'
    ) false
}

def test-issue-context []: nothing -> nothing {
    expect 'normalizes an open issue' (
        issue-context-record 42 {
            number: 42
            state: open
            user: {login: alice id: 99}
        }
    ) {number: 42, author: alice, author_id: 99}

    let invalid_records = [
        {
            name: 'rejects a pull request payload'
            issue: {
                number: 42
                state: open
                user: {login: alice, id: 99}
                pull_request: {}
            }
        }
        {
            name: 'rejects a mismatched issue number'
            issue: {
                number: 41
                state: open
                user: {login: alice, id: 99}
            }
        }
        {
            name: 'rejects a closed issue'
            issue: {
                number: 42
                state: closed
                user: {login: alice, id: 99}
            }
        }
        {
            name: 'rejects malformed author data'
            issue: {
                number: 42
                state: open
                user: {login: '', id: 0}
            }
        }
    ]

    $invalid_records | each {|case|
        let result = (try {
            issue-context-record 42 $case.issue
            'accepted'
        } catch {
            'rejected'
        })
        expect $case.name $result rejected
    } | ignore
}

def test-issue-number []: nothing -> nothing {
    expect 'parses a positive integer issue number' (parse-issue-number '42') 42

    [
        '1.5'
        '1e3'
        '0'
        '-1'
        'abc'
        ''
    ] | each {|value|
        let result = (try {
            parse-issue-number $value
            'accepted'
        } catch {
            'rejected'
        })
        expect $"rejects invalid issue number ($value)" $result rejected
    } | ignore
}

def test-forced-issue-implementation []: nothing -> nothing {
    let result = '{"decision":"close","priority":"priority:low","implementation":"none","reason":"The request is low impact."}'
    let automatic_verdict = issue-verdict-record $result false
    let verdict = issue-verdict-record $result false --force-implementation

    (expect
        'keeps automatic low-priority implementation disabled'
        $automatic_verdict.implementation
        none
    )
    (expect
        'keeps automatic closure safety in place'
        $automatic_verdict.decision
        needs_human
    )
    expect 'keeps a manually forced issue open' $verdict.decision keep_open
    (expect
        'preserves the triage priority for a manually forced issue'
        $verdict.priority
        'priority:low'
    )
    (expect
        'requests implementation for a manually forced issue'
        $verdict.implementation
        create_pr
    )
    expect 'preserves the model reason for a manually forced issue' (
        $verdict.reason | str contains 'The request is low impact.'
    ) true
    expect 'does not retain the automatic-closure review warning in forced mode' (
        $verdict.reason | str contains 'maintainer review is required'
    ) false
}

def test-contribution-gate-comment []: nothing -> nothing {
    let comment = {
        user: {login: 'github-actions[bot]'}
        body: 'Automated result. <!-- pullfrog-contribution-gate -->'
    }
    expect 'recognizes the contribution gate comment' (
        is-contribution-gate-comment $comment
    ) true
    expect 'rejects a comment from another author' (
        is-contribution-gate-comment ($comment | upsert user.login pullfrog)
    ) false
    expect 'rejects a comment without the marker' (
        is-contribution-gate-comment ($comment | upsert body 'Automated result.')
    ) false
}

def main [] {
    test-coauthor-validation
    test-coauthor-email
    test-prompt-rendering
    test-existing-implementation-pull-request
    test-implementation-result
    test-implementation-publication
    test-issue-context
    test-issue-number
    test-forced-issue-implementation
    test-contribution-gate-comment
    print 'contribution-gate Nushell tests passed.'
}
