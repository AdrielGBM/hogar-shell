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
        hogar-shell = final.callPackage ./nix/package.nix { src = self; };
      };

      packages = forAllSystems (system: rec {
        hogar-shell = nixpkgs.legacyPackages.${system}.callPackage ./nix/package.nix { src = self; };
        default = hogar-shell;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.hogar-shell ];
            packages = [
              pkgs.clippy
              pkgs.rustfmt
              pkgs.rust-analyzer
              pkgs.mold
              # What wrapProgram puts on the installed binary's PATH, and what `hogar-shell deps` probes for.
              pkgs.pipewire
              pkgs.wireplumber
              pkgs.libqalculate
              pkgs.ddcutil
              pkgs.wf-recorder
              pkgs.xdg-utils
            ];
            RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
            # A cargo build gets no rpath and wayland-sys dlopens libwayland-client; the installed binary gets this from postFixup instead. The Vulkan loader is for the hardware build of `apps/spike`, whose wgpu dlopens it — the shell itself is software-only and never loads it.
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
              pkgs.wayland
              pkgs.vulkan-loader
            ];
          };
        }
      );
    };
}
