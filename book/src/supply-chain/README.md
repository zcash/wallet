# Supply Chain Security

This section documents how Zallet applies [SLSA](https://slsa.dev) practices across its build and release pipelines. It captures the signing and attestation mechanisms currently in production and highlights the remaining gaps we plan to close in upcoming iterations.

## Key concepts

- **SLSA** (Supply-chain Levels for Software Artifacts) defines assurance levels for build integrity. We aim to ensure every artifact can be traced back to its source commit and the authenticated workflow that produced it.
- **SBOM** (Software Bill of Materials) inventories the dependencies bundled into an artifact, providing visibility into third-party components.
- **Provenance** captures how an artifact was produced—source, builder, parameters, and environment—which enables automated verification of the build pipeline.
- **Attestations** package metadata in [in-toto](https://in-toto.io/) format so other systems can verify integrity and policy compliance before consuming an artifact.
- **Cosign** (part of Sigstore) signs OCI images and attaches attestations using short-lived certificates backed by GitHub’s OIDC identity, eliminating the need for long-lived signing keys.

## OCI images published to Docker Hub

The workflow `.github/workflows/build-and-push-docker-hub.yaml` delegates to `zcash/.github/.github/workflows/build-and-push-docker-hub.yaml` with `sbom: true` and `provenance: true`. This pipeline:

- Builds the `zcash/zallet` image from the repository `Dockerfile` on every `vX.Y.Z` tag.
- Pushes the image to Docker Hub with tags `latest`, `vX.Y.Z`, and the exact git `sha`.
- Produces an SBOM and a provenance attestation for each OCI digest.
- Signs the image with `cosign sign`, using GitHub OIDC to obtain ephemeral certificates and records the statement in the Rekor transparency log.

Every published digest therefore ships with verifiable evidence describing what was built, with which dependencies, and under which authenticated workflow.

## Debian packages published to apt.z.cash

The workflow `.github/workflows/deb-release.yml` builds and publishes Debian packages for both `bullseye` and `bookworm`. Key steps include:

- Reproducible builds inside `rust:<distro>` containers, removing `rust-toolchain.toml` to guarantee the latest stable compiler.
- GPG signing of both packages and APT repository metadata using a Zcash-managed key decrypted via Google Cloud KMS during the job.
- Publishing the refreshed repository to the `apt.z.cash` bucket only after the packages and indices are properly signed.
- Generating build provenance attestations with `actions/attest-build-provenance`, producing an `intoto.jsonl` bundle per package.

The resulting `.deb` artifacts combine traditional GPG signatures with modern SLSA-compatible provenance metadata for downstream verification.

## Current SLSA stance

- **Build integrity:** Both pipelines run on GitHub Actions with OIDC-backed identities, aligning with SLSA Build L2+ guidance for authenticated, auditable builds that emit provenance.
- **Evidence availability:** OCI images and Debian packages expose signatures (`cosign`, GPG), SBOMs (images), and provenance (images and packages), making verification possible without shared secrets.
- **Distribution:** Signatures and attestations accompany the published artifacts (Docker Hub, GitHub Releases), easing consumption by automation and humans alike.

## Next steps

To strengthen our supply chain posture:

1. **Document verification commands.** Provide ready-to-run `cosign verify` and `cosign verify-attestation` examples with expected identities, plus instructions for verifying `.deb` files and their `intoto.jsonl` bundles.
2. **Automate downstream enforcement.** Integrate mandatory verification in deployment pipelines (e.g., `cosign verify`) and configure StageX to require the StageX lookaside signatures (`sigs.stagex.tools`) via `containers/registries.d` + `policy.json` (see [StageX signatures](https://codeberg.org/stagex/signatures)) so unsigned or mismatched images are rejected before promotion, mirroring the deterministic-release flow defined in PR #301.
3. **Expand attestation coverage.** Use GitHub’s attestation framework to emit additional statements (e.g., for CLI binaries and auxiliary artifacts) in the same way we already do for `.deb` packages.
4. **Advance deterministic builds.** Complete the deterministic build integration tracked in [PR #301](https://github.com/zcash/wallet/pull/301) so Docker images and Zallet binaries share a reproducible, hermetic toolchain.
- TODO: Track the rollout of deterministic builds from PR #301 until the deterministic image and binaries flow through the release automation end-to-end.
