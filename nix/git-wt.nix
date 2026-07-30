# `git wt` worktree wiring for the dev shell.
#
# These live in the repo's git config rather than a derivation because `git wt`
# reads them from the checkout it runs in, so entering the shell is the only
# moment we can (re)write them for the current worktree.
{
  pkgs,
  # Command that installs JS dependencies in a freshly created worktree.
  installCommand ? "just install",
}:

''
  # Set up direnv and JS dependencies whenever `git wt` creates a new
  # worktree, so worktrees are usable without a manual step.
  git config --replace-all wt.hook "direnv allow || true; ${installCommand} || true"

  # Move deleted worktree directories to the trash instead of `rm -rf`,
  # which is noticeably slower on large node_modules and target trees.
  git config --replace-all wt.remover "${pkgs.trash-cli}/bin/trash"

  # Free the worktree's direnv/nix state before it's deleted.
  git config --replace-all wt.deletehook "direnv revoke .envrc || true; ${pkgs.trash-cli}/bin/trash .direnv || true; nix store gc || true"
''
