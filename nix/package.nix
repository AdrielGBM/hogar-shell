{
  lib,
  rustPlatform,
  installShellFiles,
  pkg-config,
  makeWrapper,
  libxkbcommon,
  pipewire,
  wireplumber,
  libqalculate,
  ddcutil,
  wf-recorder,
  xdg-utils,
  src,
}:

rustPlatform.buildRustPackage {
  pname = "hyprshell";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  inherit src;

  cargoLock.lockFile = "${src}/Cargo.lock";

  nativeBuildInputs = [
    installShellFiles
    pkg-config
    makeWrapper
  ];

  buildInputs = [ libxkbcommon ];

  # The check phase relinks every test target under the release profile's fat LTO, costing more than the build itself; CI runs the suite on every push instead.
  doCheck = false;

  postInstall = ''
    installManPage ${src}/man/hyprshell.1 ${src}/man/hyprshell.5

    wrapProgram $out/bin/hyprshell \
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

  meta = {
    description = "Wayland desktop shell in Rust — bars, panels, launcher, lock screen and notifications";
    homepage = "https://github.com/AdrielGBM/hyprshell";
    license = with lib.licenses; [
      mit
      asl20
    ];
    mainProgram = "hyprshell";
    platforms = lib.platforms.linux;
  };
}
