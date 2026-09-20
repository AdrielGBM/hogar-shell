{
  lib,
  rustPlatform,
  installShellFiles,
  pkg-config,
  makeWrapper,
  libxkbcommon,
  wayland,
  pipewire,
  wireplumber,
  libqalculate,
  ddcutil,
  wf-recorder,
  xdg-utils,
  src,
  telarSrc,
}:

let
  # rustc no longer expands a `.rsx`, so nothing builds until the CLI has transpiled every package into `.telar/`.
  #
  # Built from the same checkout `[patch.crates-io]` points the library at (DEC-10), not from the published crate. The two have to move together: a CLI older than the library does not reject an attribute it has never heard of, it writes `compile_error!` into `.telar/` — so the tree stops compiling at a file nobody edited, and the real cause is a version mismatch in a tool. That happened once with `input_opaque`. This goes back to `fetchCrate` when telar is released and the patch comes out.
  cargo-telar = rustPlatform.buildRustPackage {
    pname = "cargo-telar";
    version = (lib.importTOML ../Cargo.toml).workspace.dependencies.telar.version;

    src = telarSrc;
    cargoLock = {
      lockFile = "${telarSrc}/Cargo.lock";
      # The transpiler parses Rust with rust-analyzer's own front end, which telar takes from git rather than crates.io, so vendoring it needs the tree's hash stated here. One entry covers every crate from that repository, since they share a revision.
      outputHashes = {
        "base-db-0.0.0" = "sha256-cq+wrv+Zadl1NiJIWxYKHpcXfUQXDKBYa/7eq0JWFZk=";
      };
    };
    cargoBuildFlags = [
      "-p"
      "cargo-telar"
    ];
    doCheck = false;
  };
in
rustPlatform.buildRustPackage {
  pname = "hogar-shell";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  inherit src;

  cargoLock.lockFile = "${src}/Cargo.lock";

  nativeBuildInputs = [
    cargo-telar
    installShellFiles
    pkg-config
    makeWrapper
  ];

  buildInputs = [
    libxkbcommon
    wayland
  ];

  # The check phase relinks every test target under the release profile's fat LTO, costing more than the build itself; CI runs the suite on every push instead.
  doCheck = false;

  preBuild = ''
    cargo telar transpile
  '';

  postInstall = ''
    installManPage ${src}/man/hogar-shell.1 ${src}/man/hogar-shell.5

    wrapProgram $out/bin/hogar-shell \
      --suffix PATH : ${
        lib.makeBinPath [
          pipewire
          wireplumber
          libqalculate
          ddcutil
          wf-recorder
          xdg-utils
        ]
      }
  '';

  # wayland-sys dlopens libwayland-client, so it never reaches DT_NEEDED and the fixup phase strips any rpath naming it: this has to run after that shrink, against the binary wrapProgram left behind.
  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath [ wayland ]} $out/bin/.hogar-shell-wrapped
  '';

  meta = {
    description = "Wayland desktop shell in Rust — bars, panels, launcher, lock screen and notifications";
    homepage = "https://github.com/AdrielGBM/hogar-shell";
    license = with lib.licenses; [
      mit
      asl20
    ];
    mainProgram = "hogar-shell";
    platforms = lib.platforms.linux;
  };
}
