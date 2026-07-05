{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.capsule;
  socketPathEnvironment = {
    CAPSULE_SOCKET_PATH = cfg.daemon.socketPath;
  };
  sessionSocketPathEnvironment = {
    CAPSULE_SOCKET_PATH = builtins.replaceStrings [ "%h" ] [ "$HOME" ] cfg.daemon.socketPath;
  };
  daemonEnvironment =
    cfg.daemon.environment
    // socketPathEnvironment
    // {
      CAPSULE_NIX_MANAGED = "1";
    };
  environmentList = lib.mapAttrsToList (name: value: "${name}=${value}") daemonEnvironment;
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
      enable = lib.mkEnableOption "capsule daemon managed as a systemd user service";

      socketPath = lib.mkOption {
        type = lib.types.nonEmptyStr;
        default = "%h/.capsule/capsule.sock";
        description = "Unix-domain socket path used by capsule's shell relay. systemd expands %h to the user's home directory.";
      };

      environment = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = { };
        example = lib.literalExpression ''{ XDG_CONFIG_HOME = "%h/.config"; }'';
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
      environment.sessionVariables = sessionSocketPathEnvironment;

      systemd.user.services.capsule = {
        description = "capsule prompt daemon";
        requires = [ "capsule.socket" ];
        after = [ "capsule.socket" ];

        serviceConfig = {
          Type = "simple";
          ExecStart = "${lib.getExe cfg.package} daemon";
          Environment = environmentList;
        };
      };

      systemd.user.sockets.capsule = {
        description = "capsule prompt daemon socket";
        wantedBy = [ "sockets.target" ];
        listenStreams = [ cfg.daemon.socketPath ];

        socketConfig = {
          SocketMode = "0700";
          DirectoryMode = "0700";
        };
      };
    })
  ];
}
