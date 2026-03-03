#!/bin/sh

set -e

DIR="$( cd "$( dirname "$0" )" && pwd )"
REPO_ROOT="$(git rev-parse --show-toplevel)"
PLATFORM="linux/amd64"
OCI_OUTPUT="$REPO_ROOT/build/oci"
CONTAINERFILE="$REPO_ROOT/Containerfile"

echo $CONTAINERFILE
mkdir -p $OCI_OUTPUT

# Build runtime image for podman run
echo "Building runtime image..."
podman buildx build -f "$CONTAINERFILE" "$REPO_ROOT" \
	--platform "$PLATFORM" \
	--target runtime \
	--source-date-epoch 1 \
	--rewrite-timestamp \
	--disable-compression=false \
	--tag zallet:latest \
	"$@"

echo "Exporting OCI archive..."
podman save --format oci-archive -o "$OCI_OUTPUT/zallet.tar" zallet:latest

# Extract binary locally from export stage
echo "Extracting binary..."
podman buildx build -f "$CONTAINERFILE" "$REPO_ROOT" --quiet \
	--platform "$PLATFORM" \
	--target export \
	--output type=local,dest="$REPO_ROOT/build" \
	"$@"
