{ lib, root }:
lib.fileset.toSource {
  inherit root;
  fileset = lib.fileset.unions [
    (root + /rust)
    (root + /flake.lock)
    (root + /ccusage.example.json)
    (root + /package.json)
  ];
}
