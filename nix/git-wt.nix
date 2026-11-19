{
  pkgs,
  installCommand ? "just install",
}:

''
  git config --replace-all wt.hook "direnv allow || true; ${installCommand} || true"

  git config --replace-all wt.remover "${pkgs.trash-cli}/bin/trash"

  git config --replace-all wt.deletehook "direnv revoke .envrc || true; ${pkgs.trash-cli}/bin/trash .direnv || true; nix store gc || true"
''
