{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.capsule;
  inherit (pkgs.stdenv.hostPlatform) isDarwin isLinux;
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
      enable = lib.mkEnableOption "capsule daemon managed by Home Manager";

      socketPath = lib.mkOption {
        type = lib.types.str;
        default = "${config.home.homeDirectory}/.capsule/capsule.sock";
        defaultText = lib.literalExpression ''"${config.home.homeDirectory}/.capsule/capsule.sock"'';
        description = "Unix-domain socket path used by capsule's shell relay.";
      };

      environment = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = {
          XDG_CONFIG_HOME = config.xdg.configHome;
        };
        defaultText = lib.literalExpression "{ XDG_CONFIG_HOME = config.xdg.configHome; }";
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
        {
          assertion = !cfg.daemon.enable || isDarwin || isLinux;
          message = "capsule daemon management is only supported on Darwin and Linux.";
        }
      ];
    }

    (lib.mkIf cfg.enable {
      home.packages = [ cfg.package ];
    })

    (lib.mkIf (cfg.enable && cfg.daemon.enable) {
      home.sessionVariables = socketPathEnvironment;

      home.activation.createCapsuleRuntimeDir = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
        run mkdir -m 700 -p $VERBOSE_ARG ${lib.escapeShellArg socketDir}
      '';
    })

    (lib.mkIf (cfg.enable && cfg.daemon.enable && isLinux) {
      systemd.user.services.capsule = {
        Unit = {
          Description = "capsule prompt daemon";
          Requires = [ "capsule.socket" ];
          After = [ "capsule.socket" ];
        };

        Service = {
          Type = "simple";
          ExecStart = "${lib.getExe cfg.package} daemon";
          Environment = environmentList;
        };
      };

      systemd.user.sockets.capsule = {
        Unit = {
          Description = "capsule prompt daemon socket";
        };

        Socket = {
          ListenStream = cfg.daemon.socketPath;
          SocketMode = "0700";
          DirectoryMode = "0700";
        };

        Install = {
          WantedBy = [ "sockets.target" ];
        };
      };
    })

    (lib.mkIf (cfg.enable && cfg.daemon.enable && isDarwin) {
      launchd.agents.capsule = {
        enable = true;
        domain = lib.mkDefault "gui";
        config = {
          Label = "com.github.shuymn.capsule";
          ProgramArguments = [
            (lib.getExe cfg.package)
            "daemon"
          ];
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
