{
  description = "PlexAssistant — native STRM support for Plex";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust toolchain
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            # media probing and test media generation
            ffmpeg
            # inspecting the Plex library and link cache databases
            sqlite
            # task runner and JSON inspection of the daemon's /state
            just
            jq
          ];
          # lets rust-analyzer resolve std sources
          env.RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
    };
}
