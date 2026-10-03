{ config, lib, ... }:
let
  cfg = config.programs.capsule;
in
{
  imports = [ ./common.nix ];

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    programs.zsh = lib.mkIf cfg.enableZshIntegration {
      enable = lib.mkDefault true;
      initContent = lib.mkAfter ''
        eval "$(${lib.getExe cfg.package} init zsh)"
      '';
    };
  };
}
