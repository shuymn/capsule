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
          package = self.packages.${system}.capsule;
          capsuleHome = if isDarwin then "/Users/capsule" else "/home/capsule";
          capsuleConfig = {
            programs.capsule = {
              enable = true;
              inherit package;
            };
          };
          homeConfig = {
            home = {
              username = "capsule";
              homeDirectory = capsuleHome;
              stateVersion = "26.05";
            };
          };
          homeManager =
            extra:
            (home-manager.lib.homeManagerConfiguration {
              inherit pkgs;
              modules = [
                self.homeManagerModules.default
                capsuleConfig
                homeConfig
                extra
              ];
            }).config;
          hm = homeManager { };
          legacy = homeManager { programs.capsule.daemon.enable = true; };
          legacyRejected = !(builtins.tryEval legacy.home.activationPackage.drvPath).success;
          noService =
            config:
            !(lib.hasAttrByPath [ "systemd" "user" "services" "capsule" ] config)
            && !(lib.hasAttrByPath [ "systemd" "user" "sockets" "capsule" ] config)
            && !(lib.hasAttrByPath [ "launchd" "agents" "capsule" ] config)
            && !(lib.hasAttrByPath [ "launchd" "user" "agents" "capsule" ] config);
          moduleCheck =
            name: config: packages: init:
            assert lib.assertMsg (builtins.elem package packages) "Capsule package is missing";
            assert lib.assertMsg config.programs.zsh.enable "zsh integration must enable zsh";
            assert lib.assertMsg (lib.hasInfix " init zsh" init) "Capsule zsh initialization is missing";
            assert lib.assertMsg (noService config) "Capsule must not register a service or socket";
            pkgs.runCommandLocal name { } "touch $out";
          nixos =
            (lib.nixosSystem {
              inherit system;
              modules = [
                self.nixosModules.default
                capsuleConfig
                { system.stateVersion = "26.05"; }
              ];
            }).config;
          darwin =
            (nix-darwin.lib.darwinSystem {
              inherit system;
              modules = [
                self.darwinModules.default
                capsuleConfig
                { system.stateVersion = 6; }
              ];
            }).config;
        in
        {
          inherit package;
          home-manager-module =
            assert lib.assertMsg legacyRejected "Legacy daemon options must report a migration error";
            moduleCheck "capsule-home-manager-module-check" hm hm.home.packages hm.programs.zsh.initContent;
        }
        // lib.optionalAttrs isLinux {
          nixos-module =
            moduleCheck "capsule-nixos-module-check" nixos nixos.environment.systemPackages
              nixos.programs.zsh.interactiveShellInit;
        }
        // lib.optionalAttrs isDarwin {
          darwin-module =
            moduleCheck "capsule-darwin-module-check" darwin darwin.environment.systemPackages
              darwin.programs.zsh.interactiveShellInit;
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
