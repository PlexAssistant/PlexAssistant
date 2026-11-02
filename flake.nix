{
  description = "PlexAssistant — native STRM support for Plex (Go, migrating to Rust)";

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
            go
            gotools
            gopls
            delve
            golangci-lint
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            clang
            sqlite
            ffmpeg
            just
            jq
          ];
          env.CGO_ENABLED = "1";
        };
      });
    };
}
