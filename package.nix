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
    install -Dm644 packaging/waynote.desktop \
      $out/share/applications/waynote.desktop
    install -Dm644 packaging/waynote.service \
      $out/share/systemd/user/waynote.service

    substituteInPlace $out/share/applications/waynote.desktop \
      --replace-fail 'Exec=waynote' "Exec=$out/bin/waynote"
    substituteInPlace $out/share/systemd/user/waynote.service \
      --replace-fail 'ExecStart=/usr/bin/waynote' "ExecStart=$out/bin/waynote"
  '';

  meta = {
    description = "Wayland-native, markdown-based desktop sticky notes";
    homepage = "https://github.com/mryll/waynote";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "waynote";
  };
}
