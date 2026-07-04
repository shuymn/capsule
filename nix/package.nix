{
  lib,
  rustPlatform,
  git,
  zsh,
}:

let
  manifest = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = "capsule";
  version = manifest.workspace.package.version;

  src = lib.cleanSource ../.;

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [
    "--package"
    "capsule-cli"
  ];
  cargoTestFlags = [
    "--package"
    "capsule-cli"
  ];

  nativeCheckInputs = [
    git
    zsh
  ];

  meta = {
    description = "Prompt engine for zsh";
    homepage = "https://github.com/shuymn/capsule";
    license = lib.licenses.mit;
    mainProgram = "capsule";
    platforms = lib.platforms.darwin ++ lib.platforms.linux;
  };
}
