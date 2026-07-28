# Shared helpers for the pricing lock update scripts driven by update-pricing.yaml.

# Hand the workflow both the verdict and the paths to commit, so the file list
# lives in one place instead of being repeated in the workflow.
export def report [result: record]: nothing -> nothing {
    [
        $"changed=($result.changed)"
        $"paths=($result.paths | str join ' ')"
    ]
    | each {|line| $"($line)(char nl)" | save --append $env.GITHUB_OUTPUT }
    | ignore
}
