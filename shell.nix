# Environnement de compilation de l'interface graphique :
#   nix-shell --run 'cargo build --release --features gui'
{ pkgs ? import <nixpkgs> { } }:
pkgs.mkShell {
  nativeBuildInputs = [ pkgs.pkg-config pkgs.wrapGAppsHook4 ];
  buildInputs = [ pkgs.gtk4 pkgs.libadwaita ];
  # aperçu des PDF
  packages = [ pkgs.poppler-utils ];
}
