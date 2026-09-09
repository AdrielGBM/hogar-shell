{
  lib,
  rustPlatform,
  fetchCrate,
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
}:

let
  # rustc no longer expands a `.rsx`, so nothing builds until the CLI has transpiled every package into `.telar/`. Its version is read from the manifest because the CLI refuses a tree whose `telar` it does not match.
  cargo-telar = rustPlatform.buildRustPackage (final: {
    pname = "cargo-telar";
    version = (lib.importTOML ../Cargo.toml).workspace.dependencies.telar.version;

    src = fetchCrate {
      inherit (final) pname version;
      hash = "sha256-whzxB3UQwElNvGLA7cd7mb2D63qt/Oew+J+ZJdnMY3Q=";
    };

    cargoHash = "sha256-SeLEakNC+CxXqNphVuR1KiuISSSJaIbmizBUFpC4nw8=";
  });
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
