{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell {
  nativeBuildInputs = [ pkgs.pkg-config ];
  buildInputs = with pkgs; [
    rustc
    cargo
    libopus
    alsa-lib
  ];
  RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
}
