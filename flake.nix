{
  description = "Ergo grid trading";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    naersk.url = "github:nix-community/naersk";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, naersk, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        naersk' = pkgs.callPackage naersk {};
      in
      with pkgs;
      {
        packages.default =
          naersk'.buildPackage {
            src = ./cli;
            root = ./.;

            nativeBuildInputs = with pkgs; [ pkg-config ];
            buildInputs = with pkgs; [ openssl ];
          };

        devShells.default =
          let
            escompile =
              let
                compiler-jar = fetchurl {
                  url = "https://github.com/ergoplatform/ergoscript-compiler/releases/download/v0.1/ErgoScriptCompiler-assembly-0.1.jar";
                  sha256 = "1r2bad2q271s0j1mq5yk4c9g13nd7sjwhw9b5fmq2xrw1bdr7xy4";
                };
              in
              writeShellScriptBin "escompile" ''
                ${jre}/bin/java -cp ${compiler-jar} Compile $@
              '';

            es2ergotree =
              let
                xxd = unixtools.xxd;
              in
              writeShellScriptBin "es2ergotree" ''
                if [[ $# -gt 1 ]]; then
                  echo "es2ergotree: error: too many arguments" >&2
                  echo "Usage: es2ergotree <script_dir> > output" >&2
                  exit 1
                fi
                contract="$1/contract.es"
                symbols=$([[ -f "$1/symbols.json" ]] && echo "$1/symbols.json" || echo ""])
                output="$(basename "$1").ergotree"

                ${escompile}/bin/escompile $contract $symbols \
                | head -n2                                    \
                | tail -n1                                    \
                | tr -d '\n'                                  \
                | ${xxd}/bin/xxd -r -p > $output
              '';
            rust = pkgs.rust-bin.stable."1.87.0".default.override {
              extensions = [ "rust-src" "clippy" ];
            };

          in
          mkShell {
            buildInputs = [
              openssl
              pkg-config
              rust
              rust-analyzer
              escompile
              es2ergotree
            ];
          };
      }
    );
}
