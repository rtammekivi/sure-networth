{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "sure-networth";
  version = (lib.importTOML ../Cargo.toml).package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../web
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  meta = {
    description = "Net worth allocation donut for Sure, signed in per user";
    homepage = "https://github.com/rtammekivi/sure-networth";
    license = lib.licenses.mit;
    mainProgram = "sure-networth";
    platforms = lib.platforms.unix;
  };
}
