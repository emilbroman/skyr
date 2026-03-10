{
  inputs = {
    utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      utils,
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages."${system}";
      in
      {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustup
            cargo
            gnumake
            protobuf
            lld
          ];
          shellHook = ''
            export RUSTUP_TOOLCHAIN=nightly
            rustup install nightly
            rustup component add rust-analyzer rustfmt clippy
          '';
        };
      }
    );
}
