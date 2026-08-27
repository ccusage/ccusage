use ./core.nu [COLLABORATOR_PERMISSIONS gh-api-complete repository required-env write-output]

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

def approved-capability [username: string]: nothing -> any {
    let rows = (
        open --raw .github/APPROVED_CONTRIBUTORS
        | lines
        | each {|raw_line|
            let line = $raw_line | str trim
            if ($line | is-empty) or ($line | str starts-with '#') {
                null
            } else {
                match ($line | split row -r '\s+') {
                    [$approved_username $capability] => {
                        username: ($approved_username | str lowercase)
                        capability: ($capability | str lowercase)
                    }
                    _ => null
                }
            }
        }
        | compact
    )

    $rows
    | where {|row| $row.username == ($username | str lowercase) }
    | get --optional 0
    | get --optional capability
}

def bot-author [username: string]: nothing -> bool {
    ($username | str ends-with '[bot]') or $username == 'dependabot[bot]'
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
