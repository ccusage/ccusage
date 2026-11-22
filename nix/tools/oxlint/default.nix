{ bunCli, lib }:
bunCli {
  toolDir = ./.;
  pname = "oxlint";
  wrapperArgs = ''--set OXLINT_TSGOLINT_PATH "$toolRoot/node_modules/.bin/tsrs-lint"'';
  meta = {
    description = "Lint and type-check TypeScript using the tsrs backend";
    license = lib.licenses.mit;
  };
}
