{
  pkgs,
  ...
}:

{
  languages.rust.enable = true;

  packages =
    with pkgs;
    [
      openssl
      pkg-config
      libopus
      iconv
    ]
    ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
      alsa-lib
    ];
}
