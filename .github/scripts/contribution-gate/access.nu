use ./core.nu [
    APPROVAL_CAPABILITIES
    COLLABORATOR_PERMISSIONS
    gh-api-complete
    gh-api-body
    repository
    required-env
    write-output
]

def collaborator-permission [repo: string, username: string]: nothing -> any {
    let result = (gh-api-complete [$"repos/($repo)/collaborators/($username)/permission"])
    if $result.exit_code == 0 {
        let permission = (try {
            $result.stdout | from json | get --optional permission
        } catch {
            null
        })
        let valid_permissions = [
            admin
            maintain
            write
            triage
            read
            none
        ]
        if ($permission | describe) == 'string' and $permission in $valid_permissions {
            return {status: 'ok', permission: $permission}
        }
        print --stderr $"Could not determine collaborator access: malformed permission response for ($username)"
        return {status: 'unknown', permission: null}
    }
    if ($result.stderr | str contains 'HTTP 404') {
        return {status: 'not_collaborator', permission: null}
    }
    print --stderr $"Could not determine collaborator access: ($result.stderr | str trim)"
    {status: 'unknown', permission: null}
}

export def approved-entries [content: string]: nothing -> list<record> {
    $content | lines | each {|line|
        let parts = $line | str trim | split row -r '\s+'
        match $parts {
            [$username $capability] if not ($username | str starts-with '#') and ($capability | str lowercase) in $APPROVAL_CAPABILITIES => {
                line: $line
                username: $username
                capability: ($capability | str lowercase)
            }
            _ => {line: $line, username: null, capability: null}
        }
    }
}

export def approval-capability [entries: list<record>, username: string]: nothing -> any {
    $entries
    | where {|row| $row.username != null and ($row.username | str lowercase) == ($username | str lowercase) }
    | get --optional 0.capability
}

export def approval-update [content: string, username: string, capability: string]: nothing -> record {
    if $capability not-in $APPROVAL_CAPABILITIES {
        error make {msg: 'Invalid approval capability'}
    }
    let entries = approved-entries $content
    let existing = approval-capability $entries $username
    if $existing == pr or $existing == $capability {
        return {status: already, capability: $existing, content: $content}
    }
    let rows = if $existing == null {
        (
            $entries
            | reverse
            | skip while {|row| $row.line | str trim | is-empty }
            | reverse
        )
        | append {line: $username, username: $username, capability: $capability}
    } else {
        $entries | each {|row|
            if $row.username != null and ($row.username | str lowercase) == ($username | str lowercase) {
                $row | update capability $capability
            } else { $row }
        }
    }
    let updated = $rows | each {|row|
        if $row.username == null { $row.line } else { $"($row.username) ($row.capability)" }
    } | str join "\n"
    {
        status: (if $existing == null { 'added' } else { 'updated' })
        capability: $capability
        content: ($updated | str trim --right --char "\n" | $in + "\n")
    }
}

def approved-capability [username: string]: nothing -> any {
    approval-capability (approved-entries (open --raw .github/APPROVED_CONTRIBUTORS)) $username
}

export def approve-contributor []: nothing -> nothing {
    let event = open --raw (required-env GITHUB_EVENT_PATH) | from json
    let comment = $event.comment.body | str trim
    let capability = if $comment =~ '(?i)\blgtmi\b' {
        'issue'
    } else if $comment =~ '(?i)\blgtm\b' {
        'pr'
    } else {
        write-output status skipped
        return
    }
    let permission = collaborator-permission (repository) $event.comment.user.login
    if $permission.status == unknown {
        error make {msg: 'Could not verify contributor approval permission'}
    }
    if $permission.status != ok or $permission.permission not-in $COLLABORATOR_PERMISSIONS {
        write-output status skipped
        return
    }
    let file = '.github/APPROVED_CONTRIBUTORS'
    let update = approval-update (open --raw $file) $event.issue.user.login $capability
    if $update.status != already {
        $update.content | save --force $file
    }
    write-output status $update.status
    write-output capability $update.capability
}

export def approve-contributor-comment []: nothing -> nothing {
    let event = open --raw (required-env GITHUB_EVENT_PATH) | from json
    let username = $event.issue.user.login
    let status = required-env APPROVAL_STATUS
    let capability = required-env APPROVAL_CAPABILITY
    let body = if $status == already {
        $"@($username) is already approved."
    } else {
        let message = if $capability == issue {
            $"@($username) approved for issues. Your future issues will bypass the contribution gate. PRs still require `lgtm`."
        } else {
            $"@($username) approved for issues and PRs. Your future issues and PRs will bypass the contribution gate."
        }
        let branch = $event.repository.default_branch
        [
            $message
            $"See [CONTRIBUTING.md]\(https://github.com/(repository)/blob/($branch)/CONTRIBUTING.md\)."
        ] | str join "\n\n"
    }
    gh-api-body POST $"repos/(repository)/issues/($event.issue.number)/comments" {body: $body} | ignore
}

def bot-author [username: string]: nothing -> bool {
    $username | str ends-with '[bot]'
}

def author-access [kind: string]: nothing -> record {
    let repo = repository
    let author = required-env AUTHOR

    if (bot-author $author) {
        return {
            skip: true
            close_allowed: false
            bypass: false
            author_status: 'bot'
        }
    }

    let permission = collaborator-permission $repo $author
    if $permission.status == 'unknown' {
        return {
            skip: false
            close_allowed: false
            bypass: false
            author_status: 'permission-unknown'
        }
    }
    if $permission.status == 'ok' and $permission.permission in $COLLABORATOR_PERMISSIONS {
        return {
            skip: true
            close_allowed: false
            bypass: true
            author_status: 'collaborator'
        }
    }

    let capability = approved-capability $author
    let approved = if $kind == 'issue' {
        $capability == 'issue' or $capability == 'pr'
    } else {
        $capability == 'pr'
    }

    {
        skip: $approved
        close_allowed: (not $approved)
        bypass: $approved
        author_status: (if $approved { $"approved:($capability)" } else { 'new' })
    }
}

export def issue-access []: nothing -> nothing {
    let access = author-access issue
    write-output skip ($access.skip | into string)
    write-output close_allowed ($access.close_allowed | into string)
    write-output author_status $access.author_status
    print $"Issue author status: ($access.author_status)"
}

export def pr-access []: nothing -> nothing {
    let access = author-access pr
    write-output skip ($access.skip | into string)
    write-output bypass ($access.bypass | into string)
    write-output close_allowed ($access.close_allowed | into string)
    print $"PR author status: ($access.author_status)"
}
