{
  description = "Zallet — reproducible static aarch64-musl build (arm64 release path)";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane/v0.20.3";
    rust-overlay = { url = "github:oxalica/rust-overlay"; inputs.nixpkgs.follows = "nixpkgs"; };
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, nixpkgs, crane, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; overlays = [ (import rust-overlay) ]; };
        muslTarget =
          if system == "aarch64-linux" then "aarch64-unknown-linux-musl"
          else "x86_64-unknown-linux-musl";
        rustToolchain = pkgs.rust-bin.stable."1.85.1".default.override {
          targets = [ muslTarget ];
        };
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
        ftlOrCargo = path: type:
          (builtins.match ".*\\.ftl$" path != null)
          || (builtins.match ".*/i18n\\.toml$" path != null)
          || (craneLib.filterCargoSources path type);
        src = pkgs.lib.cleanSourceWith { src = ./.; filter = ftlOrCargo; name = "source"; };
        # The *-sys C/C++ deps need a coherent musl toolchain. pkgsMusl.clangStdenv.cc
        # is a clang targeting aarch64-musl with libc++ — this resolves a two-sided
        # conflict that ad-hoc cc choices can't:
        #   * pqcrypto/pqclean (C): its compat.h does `#include <features.h>` only
        #     when `!defined(__clang__)`; clang defines __clang__ so it skips that
        #     glibc-only header (gcc would pull it and fail to compile under musl).
        #   * zcash_script (C++): clangStdenv links LLVM libc++ built against musl,
        #     instead of gcc's libstdc++ which references glibc-only symbols
        #     (__libc_single_threaded, __cxa_thread_atexit_impl) that break the
        #     static-musl link.
        # This mirrors how StageX's pre-integrated clang+libc++ pallet works on amd64.
        clangCC = pkgs.pkgsMusl.clangStdenv.cc;
        zallet = craneLib.buildPackage {
          inherit src;
          strictDeps = true;
          cargoExtraArgs = "--locked --bin zallet --features rpc-cli,zcashd-import";
          CARGO_BUILD_TARGET = muslTarget;
          # Static musl; use the musl clang as the linker so libc++ + crt resolve
          # coherently (a generic cc-wrapper mis-targets gnu vs musl here).
          CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static -C codegen-units=1 -C linker=${clangCC}/bin/cc -C link-arg=-static";
          CC_aarch64_unknown_linux_musl = "${clangCC}/bin/cc";
          CXX_aarch64_unknown_linux_musl = "${clangCC}/bin/c++";
          nativeBuildInputs = with pkgs; [ protobuf llvmPackages.clang pkg-config git ];
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          doCheck = false;
        };
      in { packages.default = zallet; packages.zallet = zallet; });
}
