#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

use std/assert

def main []: nothing -> nothing {
    let fixture = mktemp -d
    try {
        mkdir ($fixture | path join .scripts)
        "# résumé 日本語\nexport def broken []: nothing -> int { let 名前: int = \"文字\"; $名前 }\n"
        | save ($fixture | path join .scripts lib.nu)
        "use .scripts/lib.nu\n" | save ($fixture | path join clean.nu)
        "use .scripts/lib.nu\nlet y: int = \"b\"" | save ($fixture | path join last-line.nu)
        "each" | save ($fixture | path join eof.nu)
        "let z: int = \"c\"\r\nlet w: int = \"d\"\r\n" | save ($fixture | path join crlf.nu)

        let result = nu --no-config-file ($env.FILE_PWD | path join check-nushell.nu) $fixture | complete
        assert not equal $result.exit_code 0
        assert equal ($result.stderr | lines | where $it =~ '^[^ ]+\.nu:') [
            '.scripts/lib.nu:2:54: Error: Type mismatch.'
            'crlf.nu:1:14: Error: Type mismatch.'
            'crlf.nu:2:14: Error: Type mismatch.'
            'eof.nu:1:5: Error: Missing required positional argument.'
            'last-line.nu:2:14: Error: Type mismatch.'
        ]
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'Nushell diagnostic location tests passed.'
}
