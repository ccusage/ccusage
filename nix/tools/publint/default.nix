{ bunCli, lib }:
bunCli {
  toolDir = ./.;
  pname = "publint";
  meta = {
    description = "Lint packaging errors";
    homepage = "https://publint.dev";
    license = lib.licenses.mit;
  };
}
