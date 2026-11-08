{
  inputs.rustdev-nix.url = "github:yutadev31/rustdev-nix";

  outputs =
    inputs:
    inputs.rustdev-nix.lib.mkFlake {
      inherit inputs;
      src = ./.;

      packageAttrs = { pkgs, ... }: {
        postInstall = ''
          wrapProgram $out/bin/ruterm-desktop --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.fontconfig ]}
        '';

        nativeBuildInputs = [
          pkgs.pkg-config
          pkgs.makeWrapper
        ];

        buildInputs = [
          pkgs.wayland
          pkgs.libxkbcommon
          pkgs.libGL
          pkgs.vulkan-loader
          pkgs.fontconfig
        ];
      };
    };
}
