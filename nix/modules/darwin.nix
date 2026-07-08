{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.capsule;
  primaryUser = if config.system.primaryUser == null then "" else config.system.primaryUser;
  socketDir = builtins.dirOf cfg.daemon.socketPath;
  socketPathEnvironment = {
    CAPSULE_SOCKET_PATH = cfg.daemon.socketPath;
  };
  daemonEnvironment =
    cfg.daemon.environment
    // socketPathEnvironment
    // {
      CAPSULE_NIX_MANAGED = "1";
    };
in
{
  options.programs.capsule = {
    enable = lib.mkEnableOption "capsule prompt engine";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./nix/package.nix { }";
      description = "capsule package to install.";
    };

    daemon = {
      enable = lib.mkEnableOption "capsule daemon managed by nix-darwin launchd";

      socketPath = lib.mkOption {
        type = lib.types.nonEmptyStr;
        default = "${config.system.primaryUserHome}/.capsule/capsule.sock";
        defaultText = lib.literalExpression ''"${config.system.primaryUserHome}/.capsule/capsule.sock"'';
        description = "Unix-domain socket path used by capsule's shell relay.";
      };

      environment = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = {
          XDG_CONFIG_HOME = "${config.system.primaryUserHome}/.config";
        };
        defaultText = lib.literalExpression ''{ XDG_CONFIG_HOME = "${config.system.primaryUserHome}/.config"; }'';
        description = "Environment variables passed to the socket-activated daemon.";
      };
    };
  };

  config = lib.mkMerge [
    {
      assertions = [
        {
          assertion = !cfg.daemon.enable || cfg.enable;
          message = "programs.capsule.daemon.enable requires programs.capsule.enable.";
        }
      ];
    }

    (lib.mkIf cfg.enable {
      environment.systemPackages = [ cfg.package ];
    })

    (lib.mkIf (cfg.enable && cfg.daemon.enable) {
      environment.variables = socketPathEnvironment;

      system.requiresPrimaryUser = [ "programs.capsule.daemon.enable" ];

      system.activationScripts.userLaunchd.text = lib.mkBefore ''
        sudo --user=${lib.escapeShellArg primaryUser} -- mkdir -m 700 -p ${lib.escapeShellArg socketDir}
      '';

      launchd.user.agents.capsule = {
        command = "${lib.getExe cfg.package} daemon";
        serviceConfig = {
          Label = "com.github.shuymn.capsule";
          EnvironmentVariables = daemonEnvironment;
          Sockets.Listeners = {
            SockPathName = cfg.daemon.socketPath;
            SockPathMode = 448;
          };
        };
      };
    })
  ];
}
