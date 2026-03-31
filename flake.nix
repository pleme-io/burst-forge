{
  description = "burst-forge — Kubernetes burst test orchestrator";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    crate2nix.url = "github:nix-community/crate2nix";
    flake-utils.url = "github:numtide/flake-utils";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crate2nix, flake-utils, substrate, ... }: let
    # Base tool outputs (packages, devShells, release apps, overlays)
    toolOutputs = (import "${substrate}/lib/build/rust/tool-release-flake.nix" {
      inherit nixpkgs crate2nix flake-utils;
    }) {
      toolName = "burst-forge";
      src = self;
      repo = "pleme-io/burst-forge";
    };

    # Experiment flow apps — one per config file in configs/
    # Uses cargo-built binary (Cargo.nix is stale for shikumi git dep)
    experimentApps = flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs { inherit system; };

      mkBurstApp = name: configFile: {
        type = "app";
        program = toString (pkgs.writeShellScript "burst-forge-${name}" ''
          set -euo pipefail
          REPO_ROOT="$(cd "$(dirname "$0")/../.." 2>/dev/null && pwd || echo "${self}")"
          # Find burst-forge binary: cargo-built or PATH
          BIN="$REPO_ROOT/target/release/burst-forge"
          if [ ! -x "$BIN" ]; then
            BIN="$(command -v burst-forge 2>/dev/null || true)"
          fi
          if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
            echo "burst-forge not found. Run: cargo build --release" >&2
            exit 1
          fi
          export KUBECONFIG="''${KUBECONFIG:-$HOME/.kube/scale-test.yaml}"
          export CONFLUENCE_API_TOKEN="''${CONFLUENCE_API_TOKEN:-$(cat "$HOME/.config/atlassian/akeyless/api-token" 2>/dev/null || echo "")}"
          exec "$BIN" matrix --config ${configFile} "$@"
        '');
      };
    in {
      apps = {
        cerebras-matrix  = mkBurstApp "cerebras-matrix"  "${self}/configs/cerebras-matrix.yaml";
        optimized-matrix = mkBurstApp "optimized-matrix" "${self}/configs/optimized-matrix.yaml";
        original-matrix  = mkBurstApp "original-matrix"  "${self}/configs/original-matrix.yaml";
        quick-1000       = mkBurstApp "quick-1000"       "${self}/configs/single-1000.yaml";
      };
    });
  in
    # Deep merge: tool outputs + experiment apps
    toolOutputs // {
      apps = builtins.mapAttrs (system: toolApps:
        toolApps // (experimentApps.apps.${system} or {})
      ) (toolOutputs.apps or {});
    };
}
