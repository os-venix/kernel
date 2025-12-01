{
  description = "Venix kernel";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      pkgs = import nixpkgs {
        system = "x86_64-linux";
      };
    in {
      packages.x86_64-linux.kernel = pkgs.stdenv.mkDerivation {
        pname = "venix-kernel";
        version = "0.4";

        src = self;
        # or src = ./.; self is better when used via flake input

        # change these for your actual build
        buildInputs = [ pkgs.rustc pkgs.cargo pkgs.jq ];

        buildPhase = ''
          export CARGO_HOME=$PWD/.cargo-home
          # build release and capture the exact executable path from cargo messages
          cargo build --offline --release --message-format=json --target=x86_64-unknown-none | tee cargo-msgs.json
          # use jq to extract the executable path for the kernel target
          KFILE=$$(jq -r 'select(.reason=="compiler-artifact" and .target.name=="kernel" and .executable!=null).executable' cargo-msgs.json | tail -n1)
          if [ -z "$$KFILE" ]; then
            echo "Failed to find built kernel executable in cargo output"
            exit 1
          fi
          echo $$KFILE > .artefacts/kernel_path
        '';

        installPhase = ''
          KFILE=$(cat .artefacts/kernel_path)
          mkdir -p $out
          cp "$KFILE" $out/kernel
        '';
      };

      # Allow `nix build` in the kernel repo
      defaultPackage.x86_64-linux = self.packages.x86_64-linux.kernel;
    };
}
