{
  description = "Venix kernel";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      overlays = [ (import rust-overlay) ];
      system = "x86_64-linux";

      pkgs = import nixpkgs {
        inherit system overlays;
      };

      rustToolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default.override {
        extensions = [
          "rust-src"
          "rust-analyzer"
        ];
        targets = [ "x86_64-unknown-none" ];
      });

      rustPlatform = pkgs.makeRustPlatform {
        cargo = rustToolchain;
        rustc = rustToolchain;
      };

      devDeps = with pkgs; [
        make
        gcc
      ];
    in {
      packages.x86_64-linux.kernel = rustPlatform.buildRustPackage rec {
        passthru.networkAllowed = true;
        pname = "kernel";
        version = "0.4";
        src = ./.;
        cargoVendorDir = "vendor";

        doCheck = false;
        cargoBuildFlags = [ "--target" "x86_64-unknown-none" ];

        installPhase = ''
          mkdir -p $out
          cp target/x86_64-unknown-none/release/${pname} $out/kernel
        '';
      };

      defaultPackage.x86_64-linux = self.packages.x86_64-linux.kernel;
    };
}
