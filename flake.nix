{
  description = "capsule prompt engine";

  nixConfig = {
    extra-substituters = [ "https://shuymn.cachix.org" ];
    extra-trusted-public-keys = [ "shuymn.cachix.org-1:bUcNU5/B3gNbM7htHCYmKVVb1bUwNx2vc2W4aOJlloQ=" ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nix-darwin = {
      url = "github:LnL7/nix-darwin";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      home-manager,
      nix-darwin,
    }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forAllSystems = lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          capsule = pkgs.callPackage ./nix/package.nix { };
        in
        {
          inherit capsule;
          default = capsule;
        }
      );

      apps = forAllSystems (system: {
        capsule = {
          type = "app";
          program = "${self.packages.${system}.capsule}/bin/capsule";
          meta.description = "Run capsule";
        };
        default = self.apps.${system}.capsule;
      });

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          inherit (pkgs.stdenv.hostPlatform) isDarwin isLinux;

          hmConfig = home-manager.lib.homeManagerConfiguration {
            inherit pkgs;
            modules = [
              self.homeManagerModules.default
              {
                programs.capsule = {
                  enable = true;
                  daemon.enable = true;
                  package = self.packages.${system}.capsule;
                };
                home = {
                  username = "capsule";
                  homeDirectory = if isDarwin then "/Users/capsule" else "/home/capsule";
                  stateVersion = "26.05";
                };
              }
            ];
          };

          nixosModuleEval =
            let
              eval = lib.nixosSystem {
                inherit system;
                modules = [
                  self.nixosModules.default
                  {
                    programs.capsule = {
                      enable = true;
                      daemon.enable = true;
                      package = self.packages.${system}.capsule;
                    };
                    system.stateVersion = "26.05";
                  }
                ];
              };
            in
            {
              listenStream = builtins.elemAt eval.config.systemd.user.sockets.capsule.listenStreams 0;
              socketMode = eval.config.systemd.user.sockets.capsule.socketConfig.SocketMode;
              execStart = eval.config.systemd.user.services.capsule.serviceConfig.ExecStart;
              nixManaged =
                if
                  builtins.elem "CAPSULE_NIX_MANAGED=1" eval.config.systemd.user.services.capsule.serviceConfig.Environment
                then
                  "1"
                else
                  "0";
            };

          darwinModuleEval =
            let
              eval = nix-darwin.lib.darwinSystem {
                inherit system;
                modules = [
                  self.darwinModules.default
                  {
                    programs.capsule = {
                      enable = true;
                      daemon.enable = true;
                      package = self.packages.${system}.capsule;
                    };
                    system.primaryUser = "capsule";
                    users.users.capsule.home = "/Users/capsule";
                    system.stateVersion = 6;
                  }
                ];
              };
            in
            {
              socketPath = eval.config.launchd.user.agents.capsule.serviceConfig.Sockets.Listeners.SockPathName;
              sockPathMode = eval.config.launchd.user.agents.capsule.serviceConfig.Sockets.Listeners.SockPathMode;
              command = eval.config.launchd.user.agents.capsule.command;
              nixManaged =
                eval.config.launchd.user.agents.capsule.serviceConfig.EnvironmentVariables.CAPSULE_NIX_MANAGED;
            };
        in
        {
          package = self.packages.${system}.capsule;

          home-manager-module =
            pkgs.runCommandLocal "capsule-home-manager-module-check"
              {
                socketPath =
                  if isDarwin then
                    hmConfig.config.launchd.agents.capsule.config.Sockets.Listeners.SockPathName
                  else
                    hmConfig.config.systemd.user.sockets.capsule.Socket.ListenStream;
                execStart =
                  if isDarwin then
                    builtins.concatStringsSep " " hmConfig.config.launchd.agents.capsule.config.ProgramArguments
                  else
                    hmConfig.config.systemd.user.services.capsule.Service.ExecStart;
                nixManaged =
                  if isDarwin then
                    hmConfig.config.launchd.agents.capsule.config.EnvironmentVariables.CAPSULE_NIX_MANAGED
                  else if
                    builtins.elem "CAPSULE_NIX_MANAGED=1" hmConfig.config.systemd.user.services.capsule.Service.Environment
                  then
                    "1"
                  else
                    "0";
              }
              ''
                test -n "$socketPath"
                test "$nixManaged" = "1"
                case "$execStart" in
                  *"capsule daemon"*) ;;
                  *) echo "unexpected ExecStart/ProgramArguments: $execStart" >&2; exit 1 ;;
                esac
                touch "$out"
              '';
        }
        // lib.optionalAttrs isLinux {
          nixos-module =
            pkgs.runCommandLocal "capsule-nixos-module-check"
              {
                inherit (nixosModuleEval)
                  listenStream
                  socketMode
                  execStart
                  nixManaged
                  ;
              }
              ''
                test "$listenStream" = "%h/.capsule/capsule.sock"
                test "$socketMode" = "0700"
                test "$nixManaged" = "1"
                case "$execStart" in
                  *"capsule daemon"*) ;;
                  *) echo "unexpected ExecStart: $execStart" >&2; exit 1 ;;
                esac
                touch "$out"
              '';
        }
        // lib.optionalAttrs isDarwin {
          darwin-module =
            pkgs.runCommandLocal "capsule-darwin-module-check"
              {
                inherit (darwinModuleEval)
                  socketPath
                  sockPathMode
                  command
                  nixManaged
                  ;
              }
              ''
                test "$socketPath" = "/Users/capsule/.capsule/capsule.sock"
                test "$sockPathMode" = "448"
                test "$nixManaged" = "1"
                case "$command" in
                  *"capsule daemon"*) ;;
                  *) echo "unexpected command: $command" >&2; exit 1 ;;
                esac
                touch "$out"
              '';
        }
      );

      homeManagerModules = {
        capsule = ./nix/modules/home-manager.nix;
        default = self.homeManagerModules.capsule;
      };

      nixosModules = {
        capsule = ./nix/modules/nixos.nix;
        default = self.nixosModules.capsule;
      };

      darwinModules = {
        capsule = ./nix/modules/darwin.nix;
        default = self.darwinModules.capsule;
      };

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);
    };
}
