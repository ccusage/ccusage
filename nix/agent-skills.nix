{ inputs, lib, ... }:
{
  perSystem =
    { system, ... }:
    let
      pkgs = import inputs.nixpkgs { inherit system; };
      agentLib = inputs.agent-skills.lib.agent-skills;
      # A fileset reads only the skills tree; filtering from the repository root
      # copies the whole checkout into the store, so editing any file made the
      # dev shell evaluation start from scratch.
      localSkills = lib.fileset.toSource {
        root = ./..;
        fileset = ../.agents/skills;
      };
      sources = {
        local = {
          path = localSkills;
          subdir = ".agents/skills";
        };
      };
      catalog = agentLib.discoverCatalog sources;
      allowlist = agentLib.allowlistFor {
        inherit catalog sources;
        enableAll = true;
      };
      selection = agentLib.selectSkills {
        inherit catalog sources allowlist;
        skills = { };
      };
      bundle = agentLib.mkBundle {
        inherit pkgs selection;
        name = "ccusage-agent-skills-bundle";
      };
      localTargets = {
        claude = agentLib.defaultLocalTargets.claude // {
          enable = true;
          structure = "link";
        };
      };
      installLocalHook = lib.removeSuffix "\n" (
        agentLib.mkShellHook {
          inherit pkgs bundle;
          targets = localTargets;
          quiet = true;
        }
      );
      syncAgentSkills = pkgs.writeShellApplication {
        name = "sync-agent-skills";
        text = ''
          root="''${AGENT_SKILLS_ROOT:-$PWD}"
          target="$root/.claude/skills"
          if [ -d "$target" ] && [ ! -L "$target" ]; then
            echo "$target already exists as a directory." >&2
            echo "Remove it before syncing Nix-managed agent skills." >&2
            exit 1
          fi
          ${installLocalHook} "$@"
        '';
      };
    in
    {
      packages = {
        agent-skills-bundle = bundle;
        inherit syncAgentSkills;
      };
      apps.sync-agent-skills = {
        type = "app";
        program = lib.getExe syncAgentSkills;
      };
      checks.agent-skills = bundle;
    };
}
