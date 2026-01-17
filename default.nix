{ pkgs ? import <nixpkgs> { } }:

let
  manifest = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package;
in
pkgs.rustPlatform.buildRustPackage {
  pname = manifest.name;
  version = manifest.version;

  cargoLock.lockFile = ./Cargo.lock;
  src = pkgs.lib.cleanSource ./.;

  meta = {
    description = manifest.description;
    mainProgram = "snapmixer";
  };
}

