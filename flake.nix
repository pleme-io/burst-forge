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

  in
    toolOutputs;
}
