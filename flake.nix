{
  description = "kAirPods: kairpodsd user service + Plasma 6 plasmoid (org.kairpods.plasma)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    home-manager.url = "github:nix-community/home-manager";
    home-manager.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, crane, home-manager, ... }:
    let
      # Shared module defaults
      serviceName = "kairpodsd";
      plasmoidId = "org.kairpods.plasma";
    in
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          pkgs = import nixpkgs {
            inherit system;
          };

          craneLib = crane.mkLib pkgs;

          src = pkgs.lib.cleanSourceWith {
            src = ./service;
          };

          kairpodsd = craneLib.buildPackage {
            pname = serviceName;
            version = "0.1.0";
            src = pkgs.lib.cleanSourceWith {
              src = ./service;
            };
            nativeBuildInputs = with pkgs; [
              pkg-config
            ];
            buildInputs = with pkgs; [
              dbus
              bluez
            ];
          };

          # --- Plasmoid packaging ---
          # Plasma searches $XDG_DATA_DIRS/share/plasma/plasmoids/<id>.
          # By shipping the directory in the Nix store and installing it via home.packages,
          # Plasma will see it as long as XDG_DATA_DIRS includes the profile share path
          # (Home Manager/NixOS do this).
          kairpods-plasmoid = pkgs.stdenvNoCC.mkDerivation {
            pname = plasmoidId;
            version = "0.1.0";
            src = ./.;

            dontBuild = true;

            installPhase = ''
              runHook preInstall
              mkdir -p "$out/share/plasma/plasmoids"
              ln -s "$src" "$out/share/plasma/plasmoids/${pname}"
              runHook postInstall
            '';

            meta = with pkgs.lib; {
              description = "kAirPods Plasma 6 widget (plasmoid) for AirPods battery display";
              platforms = platforms.linux;
            };
          };

        in
        {
          packages = {
            default = kairpodsd;
            kairpodsd = kairpodsd;
            plasmoid = kairpods-plasmoid;
          };
        }
      )
    //
    {
      # ---------------------------
      # Home Manager module
      # ---------------------------
      homeModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.kairpods;
        in
        {
          options.services.kairpods = {
            enable = lib.mkEnableOption "kAirPods (kairpodsd user service + Plasma plasmoid)";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.kairpodsd;
              defaultText = "kAirPods kairpodsd package from this flake";
              description = "Package providing the kairpodsd binary.";
            };
            plasmoidPackage = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.plasmoid;
              defaultText = "kAirPods plasmoid package from this flake";
              description = "Package providing the org.kairpods.plasma plasmoid.";
            };
            autoStart = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Whether to enable and start the systemd --user service.";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [
              cfg.package
              cfg.plasmoidPackage
            ];

            systemd.user.services.kairpodsd = lib.mkIf cfg.autoStart {
              Unit = {
                Description = "kAirPods D-Bus Service";
                After = [ "graphical-session.target" ];
              };
              Service = {
                Type = "dbus";
                BusName = "org.kairpods";
                ExecStart = "${cfg.package}/bin/kairpodsd";
                Restart = "on-failure";
                RestartSec = "5";
                PrivateTmp = "yes";
                NoNewPrivileges = "yes";
              };
              Install = {
                WantedBy = [ "default.target" ];
              };
            };
          };
        };

      # ---------------------------
      # NixOS module
      # ---------------------------
      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.kairpods;
        in
        {
          options.services.kairpods = {
            enable = lib.mkEnableOption "kAirPods: enable Bluetooth Experimental and provide guidance for user setup";
            enableBluezExperimental = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Enable BlueZ Experimental = true (required for AirPods battery info on many setups).";
            };
          };

          config = lib.mkIf cfg.enable {
            # Your script edits /etc/bluetooth/main.conf. On NixOS, do it declaratively:
            services.bluetooth.enable = true;
            services.bluetooth.settings = lib.mkIf cfg.enableBluezExperimental {
              General = {
                Experimental = true;
              };
            };

            # Note: user must be in bluetooth group if your daemon relies on it.
            # NixOS typically has the bluetooth group; add your user like:
            # users.users.<name>.extraGroups = [ "bluetooth" ];
          };
        };
    };
}
