#!/bin/sh
# Download all source tarballs needed for the wright bootstrap.
# Usage: ./scripts/fetch-sources.sh [output_dir]
#
# Default output directory: $WRIGHT/sources (or ./sources if WRIGHT is unset)

set -eu

DEST="${1:-${WRIGHT:-$(pwd)}/sources}"
mkdir -p "$DEST"

CURL="curl -fL --connect-timeout 5 --retry 3 --retry-delay 2 --progress-bar"

# --- Source URLs ---

GNU="https://ftpmirror.gnu.org"

# Toolchain
LINUX="https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.19.tar.xz"
MUSL="https://musl.libc.org/releases/musl-1.2.5.tar.gz"
BINUTILS="$GNU/binutils/binutils-2.46.0.tar.xz"
GCC="$GNU/gcc/gcc-15.2.0/gcc-15.2.0.tar.xz"

# GCC support libraries
GMP="$GNU/gmp/gmp-6.3.0.tar.xz"
MPFR="$GNU/mpfr/mpfr-4.2.1.tar.xz"
MPC="$GNU/mpc/mpc-1.3.1.tar.gz"

# build-base
BASH="$GNU/bash/bash-5.2.21.tar.gz"
COREUTILS="$GNU/coreutils/coreutils-9.4.tar.xz"
SED="$GNU/sed/sed-4.9.tar.xz"
GREP="$GNU/grep/grep-3.11.tar.xz"
GAWK="$GNU/gawk/gawk-5.3.0.tar.xz"
FINDUTILS="$GNU/findutils/findutils-4.9.0.tar.xz"
DIFFUTILS="$GNU/diffutils/diffutils-3.10.tar.xz"
MAKE="$GNU/make/make-4.4.1.tar.gz"
PATCH="$GNU/patch/patch-2.7.6.tar.xz"
TAR="$GNU/tar/tar-1.35.tar.xz"
XZ="https://github.com/tukaani-project/xz/releases/download/v5.4.5/xz-5.4.5.tar.xz"
ZSTD="https://github.com/facebook/zstd/releases/download/v1.5.5/zstd-1.5.5.tar.gz"
FILE="https://astron.com/pub/file/file-5.45.tar.gz"

# Essential libraries
NCURSES="$GNU/ncurses/ncurses-6.4.tar.gz"
READLINE="$GNU/readline/readline-8.2.tar.gz"
ZLIB="https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
LIBRESSL="https://ftp.openbsd.org/pub/OpenBSD/LibreSSL/libressl-3.8.2.tar.gz"

# --- All URLs in order ---

URLS="
$LINUX
$MUSL
$BINUTILS
$GCC
$GMP
$MPFR
$MPC
$BASH
$COREUTILS
$SED
$GREP
$GAWK
$FINDUTILS
$DIFFUTILS
$MAKE
$PATCH
$TAR
$XZ
$ZSTD
$FILE
$NCURSES
$READLINE
$ZLIB
$LIBRESSL
"

# --- Download ---

ok=0
fail=0

for url in $URLS; do
    filename="${url##*/}"
    dest="$DEST/$filename"

    if [ -f "$dest" ]; then
        printf "skip  %s (already exists)\n" "$filename"
        ok=$((ok + 1))
        continue
    fi

    printf "fetch %s\n" "$filename"
    if $CURL -o "$dest.part" "$url"; then
        mv "$dest.part" "$dest"
        ok=$((ok + 1))
    else
        rm -f "$dest.part"
        printf "FAIL  %s\n" "$url" >&2
        fail=$((fail + 1))
    fi
done

printf "\nDone: %d ok, %d failed, destination: %s\n" "$ok" "$fail" "$DEST"
[ "$fail" -eq 0 ]
