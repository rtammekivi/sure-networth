{
  description = "sure-networth — net worth allocation donut for Sure";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in {
        packages.default = pkgs.callPackage ./nix/package.nix { };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            clippy
            rustfmt
            rust-analyzer
            kubernetes-helm
          ];
          RUST_BACKTRACE = "1";

          shellHook = ''
            if [ -t 1 ]; then
              echo "sure-networth dev shell"
              echo
              echo "  cargo test                                 run the tests"
              echo "  cargo clippy --all-targets -- -D warnings  lint"
              echo "  cargo fmt                                  format"
              echo "  cargo run -- serve                         serve on :8080"
              echo "  cargo run -- render --open                 bake a standalone page"
              echo "  helm lint charts/sure-networth-chart       lint the chart"
              echo "  nix develop .#demo                         shell for recording the demo video"
              echo
            fi
          '';
        };

        devShells.demo = pkgs.mkShell {
          packages = with pkgs; [
            chromium
            ffmpeg
            (python3.withPackages (ps: [ ps.websocket-client ]))
          ];

          shellHook = ''
            if [ -t 1 ]; then
              echo "sure-networth demo shell"
              echo
              echo "  python3 docs/record-demo.py                regenerate docs/demo.mp4"
              echo
            fi
          '';
        };
      });
}
