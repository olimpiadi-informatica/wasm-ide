{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };
  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          overlays = [ (import rust-overlay) ];
          pkgs = import nixpkgs {
            inherit system overlays;
          };
        in
        with pkgs;
        {
          devShells.default = mkShell {
            buildInputs = [
              cargo
              cargo-binutils
              wasm-pack
              trunk
              leptosfmt
              dart-sass
              binaryen
              (rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
            ];
          };
        }
      );
}
