{
  description = "A Wayland desktop shell in Rust — bars, panels, launcher, dashboard, lock screen and notifications";

  inputs.nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";

  # The transpiler has to be the one that matches the library `[patch.crates-io]` points at (DEC-10), so it is built from the same checkout rather than fetched from crates.io. A CLI older than the library does not reject an attribute it has never heard of — it writes `compile_error!` into `.telar/`, so the tree stops compiling at a file nobody edited. `git+file` rather than `path` because a path input copies the working tree, and that tree carries a 27 GB `target/`; the cost is that only what is committed in telar is seen. Comes out with the patch when telar is released.
  inputs.telar = {
    url = "git+file:///home/adrielgbm/projects/code/telar";
    flake = false;
  };

  outputs =
    { self, nixpkgs, telar }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      overlays.default = final: _prev: {
        hogar-shell = final.callPackage ./nix/package.nix {
          src = self;
          telarSrc = telar;
        };
      };

      packages = forAllSystems (system: rec {
        hogar-shell = nixpkgs.legacyPackages.${system}.callPackage ./nix/package.nix {
          src = self;
          telarSrc = telar;
        };
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
