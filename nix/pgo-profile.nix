{
  instrumented,
  llvmProfdata,
  nodejs,
  root,
  runCommand,
}:
let
  generator = root + /apps/ccusage/scripts/generate-large-fixture.ts;
  assertUsage = ./pgo-assert-usage.mjs;
in
runCommand "ccusage-pgo-profile"
  {
    nativeBuildInputs = [ nodejs ];
  }
  ''
    fixtures="$TMPDIR/fixtures"
    export HOME="$TMPDIR/home"
    export XDG_CONFIG_HOME="$TMPDIR/xdg/config" XDG_DATA_HOME="$TMPDIR/xdg/data" XDG_CACHE_HOME="$TMPDIR/xdg/cache"
    mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"

    node ${generator} \
      --output-dir "$fixtures/large/claude" \
      --codex-output-dir "$fixtures/large/codex" \
      --grok-output-dir "$fixtures/large/grok" \
      --opencode-output-dir "$fixtures/large/opencode" \
      --size-mib 96 --codex-size-mib 96 --grok-size-mib 16 --opencode-size-mib 128
    node ${generator} --output-dir "$fixtures/many-files" --claude-shape many-small-files --size-mib 32
    node ${generator} --output-dir "$fixtures/proxy" --claude-shape requestless-proxy --size-mib 32

    export LLVM_PROFILE_FILE="$TMPDIR/profraw/%m_%p.profraw"
    export LOG_LEVEL=0 TZ=UTC NO_COLOR=1 COLUMNS=200

    runs=0
    train() {
      local claude="$1" format=table
      shift
      runs=$((runs + 1))
      [[ " $* " == *" --json "* ]] && format=json
      env CLAUDE_CONFIG_DIR="$claude" CODEX_HOME="$fixtures/large/codex" GROK_HOME="$fixtures/large/grok" \
        OPENCODE_DATA_DIR="$fixtures/large/opencode" \
        ${instrumented}/bin/ccusage "$@" --offline > "$TMPDIR/report-$runs.out"
      node ${assertUsage} "$TMPDIR/report-$runs.out" "$format" || { echo "report: ccusage $*" >&2; exit 1; }
    }

    for report in daily weekly monthly session blocks; do
      train "$fixtures/large/claude" "$report" --json
      train "$fixtures/large/claude" claude "$report" --json
    done
    for report in daily monthly session; do
      train "$fixtures/large/claude" codex "$report" --json
      train "$fixtures/large/claude" opencode "$report" --json
    done
    train "$fixtures/large/claude" daily
    train "$fixtures/large/claude" claude daily
    train "$fixtures/large/claude" claude session
    train "$fixtures/large/claude" codex daily
    train "$fixtures/large/claude" opencode daily
    train "$fixtures/large/claude" opencode daily --since 20260731 --json
    train "$fixtures/large/claude" grok daily --json
    train "$fixtures/large/claude" claude daily --since 20260731 --json
    train "$fixtures/large/claude" claude daily --since 20260901 --json
    train "$fixtures/many-files" claude daily --json
    train "$fixtures/proxy" claude daily --json

    ${llvmProfdata} merge --output "$out" "$TMPDIR"/profraw/*.profraw
  ''
