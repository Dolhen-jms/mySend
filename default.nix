# Paquet Nix : les deux interfaces (mysend dans le terminal, mysend-gui en GTK4).
#   nix-build              → ./result/bin/{mysend,mysend-gui}
#   nix-env -f . -i        → installe dans le profil
# ou, dans configuration.nix :
#   environment.systemPackages = [ (pkgs.callPackage /chemin/vers/mySend { }) ];
{ pkgs ? import <nixpkgs> { } }:
pkgs.rustPlatform.buildRustPackage {
  pname = "mysend";
  version = "0.2.0";
  src = pkgs.lib.cleanSourceWith {
    src = ./.;
    filter = path: type: !(pkgs.lib.hasPrefix "result" (baseNameOf path)) && baseNameOf path != "target";
  };
  cargoLock.lockFile = ./Cargo.lock;
  buildFeatures = [ "gui" ];
  nativeBuildInputs = [ pkgs.pkg-config pkgs.wrapGAppsHook4 ];
  buildInputs = [ pkgs.gtk4 pkgs.libadwaita ];
  # ssh, rsync, sftp-server : trouvés dans le PATH de l'utilisateur ;
  # pdftoppm / pdfinfo (aperçu des PDF) : fournis, à défaut de ceux du PATH
  preFixup = ''
    gappsWrapperArgs+=(--suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.poppler-utils ]})
  '';
  doCheck = false;
  postInstall = ''
    install -Dm644 data/mysend-gui.desktop $out/share/applications/mysend-gui.desktop
  '';
}
