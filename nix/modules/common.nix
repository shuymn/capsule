{ lib, pkgs, ... }:
{
  imports = [
    (lib.mkRemovedOptionModule [ "programs" "capsule" "daemon" ] ''
      Capsule now starts one worker per zsh session. Remove programs.capsule.daemon;
      programs.capsule.enable installs the binary and enables zsh integration.
      Move environment settings to the shell and follow docs/migration.md to retire the old service.
    '')
  ];

  options.programs.capsule = {
    enable = lib.mkEnableOption "capsule prompt engine";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./nix/package.nix { }";
      description = "Capsule package to install.";
    };

    enableZshIntegration = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Initialize Capsule in interactive zsh sessions.";
    };
  };
}
