#!/bin/sh
set -eu

REPO="nicsuzor/mem"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Darwin) PLATFORM="darwin" ;;
  Linux)  PLATFORM="linux" ;;
  *) echo "Unsupported OS: ${OS}" >&2; exit 1 ;;
esac

case "${ARCH}" in
  arm64|aarch64) ARCH="aarch64" ;;
  x86_64)        ARCH="x86_64" ;;
  *) echo "Unsupported architecture: ${ARCH}" >&2; exit 1 ;;
esac

ARTIFACT="${ARCH}-${PLATFORM}"

# Get latest version or use specified version
if [ -n "${VERSION:-}" ]; then
  TAG="v${VERSION#v}"
else
  echo "Fetching latest release..."
  if command -v gh >/dev/null 2>&1; then
    TAG="$(gh release view --repo "${REPO}" --json tagName -q .tagName)"
  elif command -v curl >/dev/null 2>&1; then
    TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | cut -d'"' -f4)"
  else
    echo "Error: need curl or gh CLI" >&2; exit 1
  fi
fi

# Check if already installed at this version
CURRENT=""
if command -v pkb >/dev/null 2>&1; then
  CURRENT="$(pkb --version 2>/dev/null | awk '{print $NF}' || true)"
fi

TARGET="${TAG#v}"
if [ "${CURRENT}" = "${TARGET}" ]; then
  echo "mem ${TARGET} already installed, nothing to do."
  exit 0
fi

if [ -n "${CURRENT}" ]; then
  echo "Upgrading mem ${CURRENT} -> ${TARGET}..."
else
  echo "Installing mem ${TARGET}..."
fi

ARCHIVE="mem-${TAG}-${ARTIFACT}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}"

# Download and extract to temp dir
TMPDIR="$(mktemp -d)"
trap 'rm -rf "${TMPDIR}"' EXIT

echo "Downloading ${URL}..."
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "${URL}" -o "${TMPDIR}/${ARCHIVE}"
elif command -v wget >/dev/null 2>&1; then
  wget -q "${URL}" -O "${TMPDIR}/${ARCHIVE}"
fi

tar -xzf "${TMPDIR}/${ARCHIVE}" -C "${TMPDIR}"

# Install binary
if [ -w "${INSTALL_DIR}" ]; then
  mv "${TMPDIR}/pkb" "${INSTALL_DIR}/"
else
  echo "Need sudo to install to ${INSTALL_DIR}"
  sudo mv "${TMPDIR}/pkb" "${INSTALL_DIR}/"
fi

echo "Installed pkb to ${INSTALL_DIR}"
echo "  pkb $(command -v pkb)"
