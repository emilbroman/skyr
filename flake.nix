{
  inputs = {
    utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    utils,
  }:
    utils.lib.eachDefaultSystem (
      system: let
        pkgs = nixpkgs.legacyPackages."${system}";
      in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustup
            cargo
            gnumake
            protobuf
            opentofu
            terragrunt
            gettext
            nodejs_25
            wasm-pack
            wasm-bindgen-cli
          ];
          shellHook = ''
            export RUSTUP_TOOLCHAIN=nightly
            rustup install nightly
            rustup component add rust-analyzer rustfmt clippy
            rustup target add wasm32-unknown-unknown
          '';
        };
      }
    );
}
