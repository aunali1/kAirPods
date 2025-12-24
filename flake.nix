{
  description = "kAirPods: kairpodsd user service + Plasma 6 plasmoid (org.kairpods.plasma)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    # Rust builder with good Cargo.lock handling
    crane.url = "github:ipetkov/crane";

    # Optional, but convenient for module wiring
    home-manager.url = "github:nix-community/home-manager";
    home-manager.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, crane, home-manager, ... }:
    let
      # Shared module defaults
      serviceName = "kairpodsd";
      plasmoidId  = "org.kairpods.plasma";
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };

        craneLib = crane.mkLib pkgs;

        # --- Rust service build ---
        # Build dependencies expected by your install script: dbus + bluez + pkg-config.
        # Add openssl only if your crate uses it; remove if not needed.
        commonBuildInputs = with pkgs; [
          dbus
          bluez
        ];

        commonNativeBuildInputs = with pkgs; [
          pkg-config
        ];

        src = pkgs.lib.cleanSourceWith {
          src = ./service;
          filter = path: type:
            # Keep everything except obvious junk
            let base = builtins.baseNameOf path; in
            !(base == "target" || base == ".git" || base == "result");
        };

        kairpodsd = craneLib.buildPackage {
          pname = serviceName;
          version = "0.1.0";
          inherit src;

          # If your crate uses a workspace, you can instead set:
          # cargoToml = ./service/Cargo.toml; cargoLock = ./Cargo.lock;

          nativeBuildInputs = commonNativeBuildInputs;
          buildInputs = commonBuildInputs;

          # If you need extra env for linking, add here.
          # For example:
          # PKG_CONFIG_PATH = "${pkgs.dbus.dev}/lib/pkgconfig:${pkgs.bluez.dev}/lib/pkgconfig";
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
            cp -R ./plasmoid "$out/share/plasma/plasmoids/${plasmoidId}"
            runHook postInstall
          '';

          meta = with pkgs.lib; {
            description = "kAirPods Plasma 6 widget (plasmoid) for AirPods battery display";
            platforms = platforms.linux;
          };
        };

      in {
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
      homeManagerModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.kairpods;
        in
        {
          options.services.kairpods = {
            enable = lib.mkEnableOption "kAirPods (kairpodsd user service + Plasma plasmoid)";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.kairpodsd;
              defaultText = "kAirPods kairpodsd package from this flake";
              description = "Package providing the kairpodsd binary.";
            };
            plasmoidPackage = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.plasmoid;
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
                Description = "kAirPods daemon (AirPods battery + integration)";
                After = [ "graphical-session.target" "dbus.service" ];
                PartOf = [ "graphical-session.target" ];
              };
              Service = {
                ExecStart = "${cfg.package}/bin/kairpodsd";
                Restart = "on-failure";
                RestartSec = 1;
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
