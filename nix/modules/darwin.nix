{ config, lib, ... }:
let
  cfg = config.programs.capsule;
in
{
  imports = [ ./common.nix ];

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    programs.zsh = lib.mkIf cfg.enableZshIntegration {
      enable = lib.mkDefault true;
      interactiveShellInit = lib.mkAfter ''
        eval "$(${lib.getExe cfg.package} init zsh)"
      '';
    };
  };
}
