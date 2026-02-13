#!/bin/sh
# Build Wright for musl targets using a container
set -eu

IMAGE_NAME="wright-builder"
OUTPUT_DIR="${1:-./target/musl-release}"

echo ":: Building in Alpine container..."
podman build -t "$IMAGE_NAME" -f Containerfile .

echo ":: Extracting binaries to $OUTPUT_DIR..."
mkdir -p "$OUTPUT_DIR"

CID=$(podman create "$IMAGE_NAME")
for bin in wright wright-build wright-repo; do
    podman cp "$CID:/usr/bin/$bin" "$OUTPUT_DIR/$bin"
done
podman rm "$CID" > /dev/null

echo ":: Done. Binaries:"
file "$OUTPUT_DIR"/wright*
