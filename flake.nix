{
  description = "unemupc98 — IBM PC/AT (80286) LLE emulator devShell (Rust + Backlog.md + tooling)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # Local-first task tracker. Provides the `backlog` CLI.
    # Do NOT add `.inputs.nixpkgs.follows` here — the bun2nix build wants its pinned nixpkgs.
    backlog-md.url = "github:MrLesk/Backlog.md";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      backlog-md,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          packages = [
            backlog-md.packages.${system}.default # `backlog` — local-first task tracker
            pkgs.pre-commit
            pkgs.git

            # --- Rust toolchain (CPU core = x86jit, all crates are Rust) ---
            pkgs.rustc
            pkgs.cargo
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.cargo-deny

            # --- guest test-ROM build (hand-assembled real-mode / long-mode programs) ---
            pkgs.nasm
            pkgs.binutils # ld, objcopy for flat binaries

            # --- FreeDOS boot-floppy build (vendor/freedos/fetch.sh) ---
            pkgs.mtools # mformat/mcopy/mdir — lay down the FAT12 filesystem
            pkgs.dosfstools # mkfs.fat — FAT12 formatting

            # --- optional differential oracles (final-state / serial compare) ---
            pkgs.qemu

            # Fetching the SingleStepTests / ProcessorTests corpus (real-mode oracle)
            pkgs.jq
          ];
        };
      }
    );
}
