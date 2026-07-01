{
  description = "Zallet — reproducible static-musl builds (amd64 + arm64)";
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
        # cargo reads the per-target env var named after the UPPERCASED triple
        # with -/. → _ (e.g. CC_x86_64_unknown_linux_musl). Derive it from the
        # active target so BOTH arches get the musl-clang toolchain — hardcoding
        # the aarch64 name left x86_64-on-x86 builds without a CC for the *-sys
        # crates.
        targetEnvSuffix =
          builtins.replaceStrings [ "-" "." ] [ "_" "_" ] muslTarget;
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
        # Build the zallet binary with a given chain-data backend. `zaino` and
        # `zebra-state` are mutually exclusive Cargo features fixed at compile
        # time, so we produce one binary per backend. `binName` renames the
        # installed binary (zebra-state keeps `zallet`; zaino becomes
        # `zallet-zaino`) so one image/deb can ship both side by side. The
        # generated share tree (completions/manpages) is always the `zallet`
        # command's — it is only produced/collected for the default binary.
        mkZallet = { backend, binName, collectShare }: craneLib.buildPackage ({
          inherit src;
          pname = binName;
          strictDeps = true;
          cargoExtraArgs = "--locked --bin zallet --no-default-features --features ${backend},rpc-cli,zcashd-import";
          CARGO_BUILD_TARGET = muslTarget;
          # Static musl; use the musl clang as the linker so libc++ + crt resolve
          # coherently (a generic cc-wrapper mis-targets gnu vs musl here).
          CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static -C codegen-units=1 -C linker=${clangCC}/bin/cc -C link-arg=-static";
          nativeBuildInputs = with pkgs; [ protobuf llvmPackages.clang pkg-config git ];
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          doCheck = false;
          # zallet/build.rs (clap_complete + clap_mangen) emits completions,
          # manpages and debian-copyright into target/<triple>/release/{completions,
          # manpages} + debian-copyright. crane installs only the binary, so copy
          # those generated assets into $out/share/zallet — the deb matrix +
          # the StageX export layout expect them there. Only the default
          # (zebra-state) binary carries the share tree + keeps the `zallet` name.
          postInstall = ''
            if [ "${binName}" != "zallet" ]; then
              mv "$out/bin/zallet" "$out/bin/${binName}"
            fi
          '' + pkgs.lib.optionalString collectShare ''
            # build.rs writes completions/ + manpages/ into the cargo target dir
            # (derived from OUT_DIR's 4th ancestor → <target>/<triple>/release).
            # crane's CARGO_TARGET_DIR isn't necessarily ./target, so SEARCH for
            # the generated dirs rather than assuming a fixed path — a fixed
            # `target/<triple>/release` silently missed them under crane, which
            # left the .deb step globbing for completions/zallet.bash and failing.
            mkdir -p "$out/share/zallet"
            comp="$(find . -type d -name completions -path '*release*' 2>/dev/null | head -1)"
            mans="$(find . -type d -name manpages    -path '*release*' 2>/dev/null | head -1)"
            if [ -n "$comp" ]; then cp -r "$comp" "$out/share/zallet/completions"; else
              echo "postInstall: completions/ not found under target tree" >&2; exit 1; fi
            if [ -n "$mans" ]; then cp -r "$mans" "$out/share/zallet/manpages"; fi
            cp="$(find . -type f -name debian-copyright -path '*release*' 2>/dev/null | head -1)"
            [ -n "$cp" ] && cp "$cp" "$out/share/zallet/debian-copyright" || true
          '';
        }
        # Per-target CC/CXX for the *-sys crates' cc-rs, keyed by the active
        # triple (CC_x86_64_unknown_linux_musl on amd64, CC_aarch64_... on arm64).
        // {
          "CC_${targetEnvSuffix}" = "${clangCC}/bin/cc";
          "CXX_${targetEnvSuffix}" = "${clangCC}/bin/c++";
        });
        # Default backend (zebra-state): keeps the `zallet` binary name + owns the
        # completions/manpages share tree that the deb + StageX export consume.
        zallet = mkZallet { backend = "zebra-state"; binName = "zallet"; collectShare = true; };
        # zaino backend: additive second binary `zallet-zaino`, no share tree.
        zallet-zaino = mkZallet { backend = "zaino"; binName = "zallet-zaino"; collectShare = false; };
      in {
        packages.default = zallet;
        packages.zallet = zallet;
        packages.zallet-zaino = zallet-zaino;
      });
}
