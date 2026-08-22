{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook4,
  gtk4,
  gtk4-layer-shell,
}:

rustPlatform.buildRustPackage {
  pname = "waynote";
  version = (lib.importTOML ./Cargo.toml).package.version;
  src = lib.cleanSource ./.;
  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
  ];

  buildInputs = [
    gtk4
    gtk4-layer-shell
  ];

  postInstall = ''
    install -Dm644 assets/waynote.svg \
      $out/share/icons/hicolor/scalable/apps/waynote.svg
  '';

  meta = {
    description = "Wayland-native, markdown-based desktop sticky notes";
    homepage = "https://github.com/mryll/waynote";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "waynote";
  };
}
