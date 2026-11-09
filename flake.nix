{
  description = "A Tumblr backup tool.";

  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs @ {
    self,
    flake-parts,
    ...
  }: let
    projectName = "archivr";
  in
    flake-parts.lib.mkFlake {inherit inputs;} {
      imports = [];
      flake.overlays.rustOverlay = inputs.rust-overlay.overlays.default;
      systems = [
        "x86_64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
        "aarch64-linux"
      ];

      perSystem = {
        config,
        self',
        inputs',
        pkgs,
        system,
        ...
      }: {
        _module.args.pkgs = import inputs.nixpkgs {
          inherit system;
          overlays = [
            self.overlays.rustOverlay
          ];
        };

        formatter = pkgs.alejandra;

        packages = {
          ${projectName} = pkgs.rustPlatform.buildRustPackage {
            pname = projectName;
            version = let file = builtins.fromTOML (builtins.readFile ./Cargo.toml); in file.package.version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            # nixpkgs' cargo setup hook sets LIBSQLITE3_SYS_USE_PKG_CONFIG=1,
            # which overrides rusqlite's "bundled" feature, so the Nix build
            # links the system SQLite instead of the amalgamation.
            nativeBuildInputs = [pkgs.pkg-config];
            buildInputs = [pkgs.sqlite];
          };
          default = self'.packages.${projectName};
        };

        # `nix flake check` only evaluates packages; listing the package here
        # makes it build (and run the test suite in the sandbox) as well.
        checks.${projectName} = self'.packages.${projectName};

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rust-bin.stable.latest.default
            rust-analyzer
            cargo-nextest
            cargo-release
          ];
        };
      };

      flake = {};
    };
}
