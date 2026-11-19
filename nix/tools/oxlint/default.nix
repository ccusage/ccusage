{ bunCli, lib }:
bunCli {
  toolDir = ./.;
  pname = "oxlint";
  meta = {
    description = "Lint and type-check TypeScript using the tsrs backend";
    license = lib.licenses.mit;
  };
}
