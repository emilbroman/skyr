{
  inputs = {
    utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    utils,
  }:
    utils.lib.eachDefaultSystem (system: let
      pkgs = nixpkgs.legacyPackages."${system}";
    in {
      devShell = pkgs.mkShell {
        packages = with pkgs; [
          rustup
          cargo
        ];
        shellHook = ''
          rustup install nightly
          rustup component add rust-analyzer
        '';
      };
    });
}
