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
        devShell = pkgs.mkShell {
          packages = with pkgs; [
            rustup
            cargo
            gnumake
            protobuf
            qemu
            cdrtools
            curl
          ];
          shellHook = ''
            export RUSTUP_TOOLCHAIN=nightly
            rustup install nightly
            rustup component add rust-analyzer
          '';
        };
      }
    );
}
