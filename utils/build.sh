#!/bin/sh

set -e

DIR="$( cd "$( dirname "$0" )" && pwd )"
REPO_ROOT="$(git rev-parse --show-toplevel)"
PLATFORM="linux/amd64"
OCI_OUTPUT="$REPO_ROOT/build/oci"
CONTAINERFILE="$REPO_ROOT/Containerfile"

export CONTAINER_BUILDKIT=1
export SOURCE_DATE_EPOCH=1

echo $CONTAINERFILE
mkdir -p $OCI_OUTPUT

# Build runtime image for podman run
echo "Building runtime image..."
podman build -f "$CONTAINERFILE" "$REPO_ROOT" \
	--platform "$PLATFORM" \
	--target runtime \
	--output type=oci,rewrite-timestamp=true,force-compression=true,dest=$OCI_OUTPUT/zallet.tar,name=zallet \
	"$@"

# Extract binary locally from export stage
echo "Extracting binary..."
podman build -f "$CONTAINERFILE" "$REPO_ROOT" --quiet \
	--platform "$PLATFORM" \
	--target export \
	--output type=local,dest="$REPO_ROOT/build" \
	"$@"
