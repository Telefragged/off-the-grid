{}:
let
  rust_overlay = import (builtins.fetchTarball https://github.com/oxalica/rust-overlay/archive/master.tar.gz);
  pkgs = import <nixpkgs> { overlays = [ rust_overlay ]; };

  rust = pkgs.rust-bin.stable."1.68.0".default.override {
    extensions = [ "rust-src" "clippy" ];
  };

  escompile =
    let
      compiler-jar = pkgs.fetchurl {
        url = "https://github.com/ergoplatform/ergoscript-compiler/releases/download/v0.1/ErgoScriptCompiler-assembly-0.1.jar";
        sha256 = "1r2bad2q271s0j1mq5yk4c9g13nd7sjwhw9b5fmq2xrw1bdr7xy4";
      };

      jre = pkgs.jre;
    in
    pkgs.writeShellScriptBin "escompile" ''
      ${jre}/bin/java -cp ${compiler-jar} Compile $@
    '';

  es2ergotree =
    let
      xxd = pkgs.unixtools.xxd;
    in
    pkgs.writeShellScriptBin "es2ergotree" ''
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
in
pkgs.mkShell {
  buildInputs = with pkgs; [
    rust
    openssl
    pkg-config
    escompile
    es2ergotree
  ];
}
