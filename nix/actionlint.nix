{ actionlint }:
actionlint.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [ ./patches/actionlint-stdin.patch ];
  postBuild = (old.postBuild or "") + ''
    go test -run '^TestCmdExecutionLargeStdin$' -timeout 15s .
  '';
})
