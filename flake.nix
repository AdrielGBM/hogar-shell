{
  description = "A Wayland desktop shell in Rust — bars, panels, launcher, dashboard, lock screen and notifications";

  inputs.nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      overlays.default = final: _prev: {
        hyprshell = final.callPackage ./nix/package.nix { src = self; };
      };

      packages = forAllSystems (system: rec {
        hyprshell = nixpkgs.legacyPackages.${system}.callPackage ./nix/package.nix { src = self; };
        default = hyprshell;
      });
    };
}
