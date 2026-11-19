use std/assert

const checker = path self ../check.nu
const fixtures = path self fixtures
const expected = path self expected.txt

def check [work: path]: nothing -> nothing {
    let files = glob --no-dir ($fixtures | path join '**/*.input') | each {|file|
        let relative = $file | path relative-to $fixtures | str replace --regex '\.input$' ''
        let dest = $work | path join $relative
        mkdir ($dest | path dirname)
        cp $file $dest
        $relative
    }
    cd $work
    ^git init --quiet
    ^git add -- ...$files
    '// TODO: untracked content.' | save untracked.js
    let tracked = ^git write-tree | str trim
    let run = ^nu --no-config-file $checker | complete
    assert equal $run.exit_code 1 $run.stderr
    assert equal $run.stderr ''
    let diagnostics = open --raw $expected | lines
    assert equal ($run.stdout | lines) $diagnostics
    rm untracked.js
    let source = ^nu --no-config-file $checker --source-tree | complete
    assert equal $source.exit_code 1 $source.stderr
    assert equal $source.stderr ''
    assert equal ($source.stdout | lines) $diagnostics
    let report = ^nu --no-config-file $checker --source-tree --report-only | complete
    assert equal $report.exit_code 0 $report.stderr
    assert equal $report.stderr ''
    assert equal ($report.stdout | lines) $diagnostics
    for file in $files {
        rm $file
    }
    let clean = ^nu --no-config-file $checker | complete
    assert equal $clean.exit_code 0 $clean.stderr
    assert equal $clean.stdout ''
    let revision = ^nu --no-config-file $checker --ref $tracked | complete
    assert equal $revision.exit_code 1 $revision.stderr
    assert equal $revision.stderr ''
    assert equal ($revision.stdout | lines) $diagnostics
}

def main []: nothing -> nothing {
    let work = mktemp -d
    try {
        check $work
    } catch {|err|
        rm -rf $work
        error make $err
    }
    rm -rf $work
    print 'Comment checker tests passed.'
}
