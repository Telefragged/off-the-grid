{}:
let
  rust_overlay = import (builtins.fetchTarball https://github.com/oxalica/rust-overlay/archive/master.tar.gz);
  pkgs = import <nixpkgs> { overlays = [ rust_overlay ]; };

  rust = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default.override {
    extensions = [ "rust-src" "clippy" ];
  });
in pkgs.mkShell {
  buildInputs = with pkgs; [
    rust
    openssl
    pkg-config
  ];
}
