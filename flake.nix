{
  description = "rune — TUI markdown editor that protects your words";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ];
      eachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = eachSystem (pkgs: rec {
        rune = pkgs.rustPlatform.buildRustPackage {
          pname = "rune";
          version = (builtins.fromTOML (builtins.readFile ./crates/rune-cli/Cargo.toml)).package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "-p" "rune-cli" ];
          doCheck = false;
          meta = {
            description = "TUI markdown editor that protects your words";
            homepage = "https://github.com/aka-rider/rune";
            license = nixpkgs.lib.licenses.mit;
            mainProgram = "rune";
          };
        };
        default = rune;
      });
    };
}
