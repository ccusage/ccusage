use ./core.nu [gh-api-json issue-number repository write-output]

export def issue-context-record [requested_number: int, issue: record]: nothing -> record {
    if $requested_number <= 0 {
        error make {msg: 'Issue number must be positive'}
    }
    if ($issue | get --optional pull_request) != null {
        error make {msg: $"#($requested_number) is a pull request, not an issue"}
    }

    let actual_number = $issue | get --optional number
    if ($actual_number | describe) != 'int' or $actual_number != $requested_number {
        error make {msg: $"GitHub returned an unexpected issue number for #($requested_number)"}
    }
    if ($issue | get --optional state) != 'open' {
        error make {msg: $"Issue #($requested_number) is not open"}
    }

    let author = $issue | get --optional user.login
    let author_id = $issue | get --optional user.id
    if ($author | describe) != 'string' or ($author | str trim | is-empty) {
        error make {msg: $"Issue #($requested_number) has an invalid author login"}
    }
    if ($author_id | describe) != 'int' or $author_id <= 0 {
        error make {msg: $"Issue #($requested_number) has an invalid author ID"}
    }

    {number: $actual_number, author: $author, author_id: $author_id}
}

export def require-open-issue []: nothing -> record {
    let repo = repository
    let number = issue-number
    let issue = gh-api-json [$"repos/($repo)/issues/($number)"]
    issue-context-record $number $issue
}

export def issue-context []: nothing -> nothing {
    let context = require-open-issue

    write-output number ($context.number | into string)
    write-output author $context.author
    write-output author_id ($context.author_id | into string)
}
