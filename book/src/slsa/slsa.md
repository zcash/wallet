# Supply Chain Security (SLSA)

Zallet’s release automation is designed to satisfy the latest [SLSA v1.0](https://slsa.dev/spec/v1.0) “Build L3” expectations: every artifact is produced on GitHub Actions with an auditable workflow identity, emits a provenance statement, and is reproducible thanks to the [StageX](https://codeberg.org/stagex/stagex/) deterministic toolchain already integrated into this repository. This page documents how the workflows operate and provides the exact commands required to validate the resulting images, binaries, attestations, and repository metadata.

## Release architecture overview

### Workflows triggered on a `vX.Y.Z` tag

- **`.github/workflows/release.yml`** orchestrates the full release. It computes metadata (`set_env`), builds the StageX-based image (`container` job), and then fan-outs to the binaries-and-Debian job (`binaries_release`) before publishing all deliverables on the tagged GitHub Release.
- **`.github/workflows/build-and-push-docker-hub.yaml`** builds the OCI image deterministically, exports runtime artifacts per platform, pushes to Docker Hub, signs the digest with Cosign (keyless OIDC), uploads the SBOM, and generates provenance via `actions/attest-build-provenance`.
- **`.github/workflows/binaries-and-deb-release.yml`** consumes the exported binaries, performs smoke tests inside Debian containers, emits standalone binaries plus `.deb` packages, GPG-signs everything with the Zcash release key (decrypted from Google Cloud KMS), generates SPDX SBOMs, and attaches `intoto.jsonl` attestations for both the standalone binary and the `.deb`.
- **StageX deterministic build** is invoked before these workflows through `make build`/`utils/build.sh`. The Dockerfile’s `export` stage emits the exact binaries consumed later, guaranteeing that the images, standalone binaries, and Debian packages share the same reproducible artifacts.

### Deliverables and metadata per release

| Artifact | Where it ships | Integrity evidence |
| --- | --- | --- |
| Multi-arch OCI image (`docker.io/<namespace>/zallet-test`) | Docker Hub | Cosign signature, Rekor entry, auto-pushed SLSA provenance, SBOM |
| Exported runtime bundle | GitHub Actions artifact (`zallet-runtime-oci-*`) | Detached from release, referenced for auditing |
| Standalone binaries (`zallet-${VERSION}-linux-{amd64,arm64}`) | GitHub Release assets | GPG `.asc`, SPDX SBOM, `intoto.jsonl` provenance |
| Debian packages (`zallet_${VERSION}_{amd64,arm64}.deb`) | GitHub Release assets + apt.z.cash | GPG `.asc`, SPDX SBOM, `intoto.jsonl` provenance |
| APT repository | Uploaded to apt.z.cash | APT `Release.gpg`, package `.asc`, cosigned source artifacts |

## Targeted SLSA guarantees

- **Builder identity:** GitHub Actions workflows run with `permissions: id-token: write`, enabling keyless Sigstore certificates bound to the workflow path (`https://github.com/zcash/zallet/.github/workflows/<workflow>.yml@refs/tags/vX.Y.Z`).
- **Provenance predicate:** `actions/attest-build-provenance@v3` emits [`https://slsa.dev/provenance/v1`](https://slsa.dev/provenance/v1) predicates for every OCI image, standalone binary, and `.deb`. Each predicate captures the git tag, commit SHA, Docker/StageX build arguments, and resolved platform list.
- **Reproducibility:** StageX already enforces deterministic builds with source-bootstrapped toolchains. Re-running `make build` in a clean tree produces bit-identical images whose digests match the published release digest.

## Verification playbook

The following sections cover every command required to validate a tagged release end-to-end (similar to [Argo CD’s signed release process](https://argo-cd.readthedocs.io/en/stable/operator-manual/signed-release-assets/), but tailored to the Zallet workflows and the SLSA v1.0 predicate).

### Tooling prerequisites

- `cosign` ≥ 2.1 (Sigstore verification + SBOM downloads)
- `rekor-cli` ≥ 1.2 (transparency log inspection)
- `crane` or `skopeo` (digest lookup)
- `oras` (optional SBOM pull)
- `slsa-verifier` ≥ 2.5
- `gh` CLI (or `curl`) for release assets
- `jq`, `coreutils` (`sha256sum`)
- `gnupg`, `gpgv`, and optionally `dpkg-sig`
- Docker 25+ with containerd snapshotter (matches the CI setup) for deterministic rebuilds

Example installation on Debian/Ubuntu:

```bash
sudo apt-get update && sudo apt-get install -y jq gnupg coreutils
go install -v github.com/sigstore/rekor/cmd/rekor-cli@latest
go install github.com/sigstore/cosign/v2/cmd/cosign@latest
go install github.com/google/go-containerregistry/cmd/crane@latest
export PATH="$PATH:$HOME/go/bin"
```

### Environment bootstrap

```bash
export VERSION=v1.2.3
export REPO=zcash/zallet
export IMAGE=docker.io/<namespace>/zallet-test               # replace <namespace> with the Docker Hub org stored in DOCKERHUB_REGISTRY (must match IMAGE_FULL_NAME from the release workflow output)
export IMAGE_WORKFLOW="https://github.com/${REPO}/.github/workflows/build-and-push-docker-hub.yaml@refs/tags/${VERSION}"
export BIN_WORKFLOW="https://github.com/${REPO}/.github/workflows/binaries-and-deb-release.yml@refs/tags/${VERSION}"
export OIDC_ISSUER="https://token.actions.githubusercontent.com"
export IMAGE_PLATFORMS="linux/amd64"                         # comma or space separated list from the release metadata
export BINARY_SUFFIXES="linux-amd64 linux-arm64"             # list of standalone binary suffixes (set to whichever assets the release produced)
export DEB_ARCHES="amd64 arm64"                              # list of Debian package architectures (match the release output)
export BIN_SIGNER_WORKFLOW="github.com/${REPO}/.github/workflows/binaries-and-deb-release.yml@refs/tags/${VERSION}"
mkdir -p verify/dist
export PATH="$PATH:$HOME/go/bin"

# Tip: running the commands below inside `bash <<'EOF' … EOF` helps keep failures isolated,
# but the snippets now return with `false` so an outer shell stays alive even without it.

# Double-check that `${IMAGE}` points to the exact repository printed by the release workflow
# (e.g. `docker.io/electriccoinco/zallet-test`). If the namespace is wrong, `cosign download`
# will look at a different repository and report "no signatures associated" even though the
# tagged digest was signed under the real namespace.
```

### 1. Validate the git tag

```bash
git fetch origin --tags
git checkout "${VERSION}"
git verify-tag "${VERSION}"
git rev-parse HEAD
```

Confirm that the commit printed by `git rev-parse` matches the `subject.digest.gitCommit` recorded in every provenance file (see section **6**).

### 2. Verify the OCI image pushed to Docker Hub

```bash
export IMAGE_DIGEST=$(crane digest "${IMAGE}:${VERSION}")
cosign verify \
  --certificate-identity "${IMAGE_WORKFLOW}" \
  --certificate-oidc-issuer "${OIDC_ISSUER}" \
  --output json \
  "${IMAGE}@${IMAGE_DIGEST}" | tee verify/dist/image-cosign.json

cosign verify-attestation \
  --type https://slsa.dev/provenance/v1 \
  --certificate-identity "${IMAGE_WORKFLOW}" \
  --certificate-oidc-issuer "${OIDC_ISSUER}" \
  --output json \
  "${IMAGE}@${IMAGE_DIGEST}" | tee verify/dist/image-attestation.json

jq -r '.payload' \
  verify/dist/image-attestation.json | base64 -d \
  > verify/dist/zallet-${VERSION}-image.slsa.intoto.jsonl

for platform in ${IMAGE_PLATFORMS//,/ }; do
  platform="$(echo "${platform}" | xargs)"
  [ -z "${platform}" ] && continue
  platform_tag="${platform//\//-}"
  cosign verify-attestation \
    --type spdxjson \
    --certificate-identity "${IMAGE_WORKFLOW}" \
    --certificate-oidc-issuer "${OIDC_ISSUER}" \
    --output json \
    "${IMAGE}@${IMAGE_DIGEST}" | tee "verify/dist/image-sbom-${platform_tag}.json"

  jq -r '.payload' \
    "verify/dist/image-sbom-${platform_tag}.json" | base64 -d \
    > "verify/dist/zallet-${VERSION}-image-${platform_tag}.sbom.spdx.json"
done

# Docker Hub does not store Sigstore transparency bundles alongside signatures,
# so the Cosign JSON output typically does NOT contain Bundle.Payload.logIndex.
# Instead, we recover the Rekor entry by searching for the image digest.

digest_no_prefix="${IMAGE_DIGEST#sha256:}"

rekor_uuid="$(
  rekor-cli search \
    --sha "${digest_no_prefix}" \
    --format json | jq -r '.UUIDs[0]'
)"

if [[ -z "${rekor_uuid}" || "${rekor_uuid}" == "null" ]]; then
  echo "Unable to locate Rekor entry for digest ${IMAGE_DIGEST} – stop verification here." >&2
  false
fi

rekor-cli get --uuid "${rekor_uuid}"
```

Cosign v3 removed the deprecated `--rekor-output` flag, so the JSON emitted by
`cosign verify --output json` is now the canonical way to inspect the verification
result. When the registry supports Sigstore transparency bundles, Cosign can expose
the Rekor log index directly under `optional.Bundle.Payload.logIndex`, but Docker Hub
does not persist those bundles, so the `optional` section is usually empty.

Because of that, the Rekor entry is recovered by searching for the image’s content
digest instead:

* `rekor-cli search --sha <digest>` returns the list of matching UUIDs.
* `rekor-cli get --uuid <uuid>` retrieves the full transparency log entry, including
  the Fulcio certificate, signature and integrated timestamp.

If the Rekor search returns no UUIDs for the digest, verification must stop, because
there is no transparency log entry corresponding to the signed image. In that case,
inspect the “Build, Attest, Sign and publish Docker Image” workflow and confirm that
the **“Cosign sign image by digest (keyless OIDC)”** step ran successfully for this
tag and digest.

The attestation verifier now expects the canonical SLSA predicate URI
(`https://slsa.dev/provenance/v1`), which distinguishes the SLSA statement from the
additional `https://sigstore.dev/cosign/sign/v1` bundle shipped alongside the image.
Cosign 3.0 returns the attestation envelope directly from `cosign verify-attestation`,
so the instructions above capture that JSON and decode the `payload` field instead of
calling `cosign download attestation`. SBOM validation reuses the same mechanism with
the `spdxjson` predicate and a `platform` annotation, so the loop above verifies and
decodes each per-platform SBOM attestation.

The SBOMs verified here are the same artifacts generated during the build
(`sbom: true`). You can further inspect them with tools like `jq` or `syft` to validate
dependencies and policy compliance.


The downloaded SBOM is generated directly by the build (`sbom: true`). Inspect it with `jq` or `syft` to validate dependencies.

### 3. Verify standalone binaries exported from the StageX image

```bash
gh release download "${VERSION}" --repo "${REPO}" \
  --pattern "zallet-${VERSION}-linux-*" \
  --dir verify/dist

curl -sSf https://apt.z.cash/zcash.asc | gpg --import -

for arch in ${BINARY_SUFFIXES//,/ }; do
  arch="$(echo "${arch}" | xargs)"
  [ -z "${arch}" ] && continue

  artifact="verify/dist/zallet-${VERSION}-${arch}"

  echo "Verifying GPG signature for ${artifact}..."
  gpg --verify "${artifact}.asc" "${artifact}"

  echo "Computing SHA256 for ${artifact}..."
  sha256sum "${artifact}" | tee "${artifact}.sha256"

  echo "Verifying GitHub SLSA provenance attestation for ${artifact}..."
  gh attestation verify "${artifact}" \
    --repo "${REPO}" \
    --predicate-type "https://slsa.dev/provenance/v1" \
    --signer-workflow "${BIN_SIGNER_WORKFLOW}"

  echo
done
```


```bash
grep -F "PackageChecksum" "verify/dist/zallet-${VERSION}-linux-amd64.sbom.spdx"
```

### 4. Verify Debian packages before consumption or mirroring

```bash
gh release download "${VERSION}" --repo "${REPO}" \
  --pattern "zallet_${VERSION}_*.deb*" \
  --dir verify/dist

for arch in ${DEB_ARCHES//,/ }; do
  arch="$(echo "${arch}" | xargs)"
  [ -z "${arch}" ] && continue

  deb="verify/dist/zallet_${VERSION}_${arch}.deb"

  echo "Verifying GPG signature for ${deb}..."
  gpg --verify "${deb}.asc" "${deb}"

  echo "Inspecting DEB metadata for ${deb}..."
  dpkg-deb --info "${deb}" | head

  echo "Computing SHA256 for ${deb}..."
  sha256sum "${deb}" | tee "${deb}.sha256"

  echo "Verifying GitHub SLSA provenance attestation for ${deb}..."
  gh attestation verify "${deb}" \
    --repo "${REPO}" \
    --predicate-type "https://slsa.dev/provenance/v1" \
    --signer-workflow "${BIN_SIGNER_WORKFLOW}"

  echo
done
```

The `.deb` SBOM files (`.sbom.spdx`) capture package checksums; compare them with `sha256sum zallet_${VERSION}_${arch}.deb`.

### 5. Validate apt.z.cash metadata

```bash
# 1. Get the Zcash signing key
curl -sSfO https://apt.z.cash/zcash.asc

# 2. Turn it into a keyring file in .gpg format
gpg --dearmor < zcash.asc > zcash-apt.gpg

# 3. Verify both dists using that keyring
for dist in bullseye bookworm; do
  curl -sSfO "https://apt.z.cash/dists/${dist}/Release"
  curl -sSfO "https://apt.z.cash/dists/${dist}/Release.gpg"
  gpgv --keyring ./zcash-apt.gpg "Release.gpg" "Release"
  grep -A3 zallet "Release"
done
```

This ensures the repository metadata match the GPG key decrypted inside the `binaries-and-deb-release` workflow.

### 6. Inspect provenance predicates (SLSA v1.0)

For any provenance file downloaded above:

e.g
```bash
FILE=verify/dist/zallet_${VERSION}_amd64.deb

# 1) Builder ID
jq -r '.predicate.runDetails.builder.id' "${FILE}.intoto.jsonl"

# 2) Version (from the workflow ref)
jq -r '.predicate.buildDefinition.externalParameters.workflow.ref
       | sub("^refs/tags/"; "")' "${FILE}.intoto.jsonl"

# 3) Git commit used for the build
jq -r '.predicate.buildDefinition.resolvedDependencies[]
       | select(.uri | startswith("git+"))
       | .digest.gitCommit' "${FILE}.intoto.jsonl"

# 4) Artifact digest from provenance
jq -r '.subject[].digest.sha256' "${FILE}.intoto.jsonl"
```

Cross-check that:

- `builder.id` matches the workflow that produced the artifact (`${IMAGE_WORKFLOW}` for OCI images, `${BIN_WORKFLOW}` for standalone binaries and `.deb` packages).
- `subject[].digest.sha256` matches the artifact’s `sha256sum`. (e.g image digest)
- `materials[].digest.sha1` equals the `git rev-parse` result from Step 1.

Automated validation:

```bash
  gh attestation verify "${FILE}" \
    --repo "${REPO}" \
    --predicate-type "https://slsa.dev/provenance/v1" \
    --signer-workflow "${BIN_SIGNER_WORKFLOW}"
```

### 7. Reproduce the deterministic StageX build locally

```bash
git clean -fdx
git checkout "${VERSION}"
make build IMAGE_TAG="${VERSION}"
skopeo inspect docker-archive:build/oci/zallet.tar | jq -r '.Digest'
```

Compare the digest returned by `skopeo` (or `docker image inspect`) with `${IMAGE_DIGEST}` from Step 2. Because StageX enforces hermetic toolchains (`utils/build.sh`), the digests must match bit-for-bit. After importing:

```bash
make import IMAGE_TAG="${VERSION}"
docker run --rm zallet:${VERSION} zallet --version
```

Running this reproduction as part of downstream promotion pipelines provides additional assurance that the published image and binaries stem from the deterministic StageX build.

## Residual work

- Extend the attestation surface (e.g., SBOM attestations, vulnerability scans) if higher SLSA levels or in-toto policies are desired downstream.
