#!/usr/bin/env bash
set -euo pipefail

# codex-switch installer / uninstaller for macOS and Linux
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/xjoker/codex-switch/master/scripts/install.sh | bash
#   curl -fsSL .../install.sh | bash -s -- --dev          # install latest dev build
#   curl -fsSL .../install.sh | bash -s -- --uninstall    # uninstall codex-switch
#   CS_VERSION=0.0.11 curl -fsSL .../install.sh | bash    # install specific version

REPO="xjoker/codex-switch"
INSTALL_DIR="/usr/local/bin"
BINARY_NAME="codex-switch"
DATA_DIR="${HOME}/.codex-switch"

info()  { printf '\033[0;34m[info]\033[0m  %s\n' "$*"; }
warn()  { printf '\033[0;33m[warn]\033[0m  %s\n' "$*" >&2; }
error() { printf '\033[0;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }

# Parse arguments
USE_DEV=false
UNINSTALL=false
for arg in "$@"; do
  case "$arg" in
    --dev)       USE_DEV=true ;;
    --uninstall) UNINSTALL=true ;;
    *)           error "Unknown argument: $arg" ;;
  esac
done

# ── Uninstall ────────────────────────────────────────────
if [ "$UNINSTALL" = true ]; then
  info "Uninstalling codex-switch..."

  SERVICE_UNINSTALL_FAILED=false
  DAEMON_BIN="$(command -v codex-switch 2>/dev/null || true)"
  if [ -z "$DAEMON_BIN" ] && [ -x "${INSTALL_DIR}/${BINARY_NAME}" ]; then
    DAEMON_BIN="${INSTALL_DIR}/${BINARY_NAME}"
  fi
  if [ -n "$DAEMON_BIN" ]; then
    if "$DAEMON_BIN" daemon uninstall; then
      info "Removed daemon service."
    else
      warn "Failed to remove daemon service with '${DAEMON_BIN} daemon uninstall'."
      SERVICE_UNINSTALL_FAILED=true
    fi
  else
    case "$(uname -s)" in
      Darwin)
        PLIST_PATH="${HOME}/Library/LaunchAgents/com.codex-switch.daemon.plist"
        if [ -f "$PLIST_PATH" ]; then
          if ! launchctl unload "$PLIST_PATH"; then
            warn "Failed to unload LaunchAgent ${PLIST_PATH}."
            SERVICE_UNINSTALL_FAILED=true
          else
            rm -f "$PLIST_PATH"
            info "Removed LaunchAgent ${PLIST_PATH}."
          fi
        fi
        ;;
      Linux)
        UNIT_PATH="${HOME}/.config/systemd/user/codex-switch-daemon.service"
        if [ -f "$UNIT_PATH" ]; then
          if ! systemctl --user disable --now codex-switch-daemon; then
            warn "Failed to disable systemd user service codex-switch-daemon."
            SERVICE_UNINSTALL_FAILED=true
          else
            rm -f "$UNIT_PATH"
            systemctl --user daemon-reload || warn "Failed to reload systemd user units."
            info "Removed systemd user service ${UNIT_PATH}."
          fi
        fi
        ;;
    esac
  fi

  if [ "$SERVICE_UNINSTALL_FAILED" = true ]; then
    error "Daemon service cleanup failed; binary and data were kept. Resolve the service error and retry uninstall."
  fi

  # Check for Homebrew install
  BREW_BIN="$(command -v codex-switch 2>/dev/null || true)"
  if [ -n "$BREW_BIN" ]; then
    RESOLVED="$(readlink -f "$BREW_BIN" 2>/dev/null || realpath "$BREW_BIN" 2>/dev/null || echo "$BREW_BIN")"
    case "$RESOLVED" in
      */Cellar/codex-switch/*|*/Homebrew/*)
        info "Homebrew installation detected. Running: brew uninstall codex-switch"
        brew uninstall codex-switch || error "brew uninstall failed"
        info "Homebrew package removed."
        # Skip direct-install removal — Homebrew was the only install method
        BREW_REMOVED=true
        ;;
    esac
  fi

  # Remove direct-install binary (skip if we just removed the Homebrew package)
  if [ "${BREW_REMOVED:-false}" != true ]; then
    BIN_PATH="${INSTALL_DIR}/${BINARY_NAME}"
    if [ -f "$BIN_PATH" ]; then
      if [ -w "$INSTALL_DIR" ]; then
        rm -f "$BIN_PATH"
      else
        info "Removing ${BIN_PATH} (requires sudo)"
        sudo rm -f "$BIN_PATH"
      fi
      info "Removed ${BIN_PATH}"
    fi
  fi

  # Remove data directory
  if [ -d "$DATA_DIR" ]; then
    printf '%s' "[info]  Remove data directory ${DATA_DIR}? [y/N] "
    read -r answer < /dev/tty 2>/dev/null || answer="n"
    case "$answer" in
      [yY]|[yY][eE][sS])
        rm -rf "$DATA_DIR"
        info "Removed ${DATA_DIR}"
        ;;
      *)
        info "Kept ${DATA_DIR}"
        ;;
    esac
  fi

  info "codex-switch has been uninstalled."
  exit 0
fi

# ── Install ──────────────────────────────────────────────

# Detect OS and architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
  linux)  PLATFORM="linux" ;;
  darwin) PLATFORM="darwin" ;;
  *)      error "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
  x86_64|amd64)   ARCH_NAME="amd64" ;;
  aarch64|arm64)   ARCH_NAME="arm64" ;;
  *)               error "Unsupported architecture: $ARCH" ;;
esac

# Check for Homebrew-installed codex-switch
BREW_BIN="$(command -v codex-switch 2>/dev/null || true)"
if [ -n "$BREW_BIN" ]; then
  RESOLVED="$(readlink -f "$BREW_BIN" 2>/dev/null || realpath "$BREW_BIN" 2>/dev/null || echo "$BREW_BIN")"
  case "$RESOLVED" in
    */Cellar/codex-switch/*|*/Homebrew/*)
      error "codex-switch is installed via Homebrew ($BREW_BIN). Please run 'brew uninstall codex-switch' first, then re-run this installer."
      ;;
  esac
fi

ASSET_NAME="cs-${PLATFORM}-${ARCH_NAME}.tar.gz"

# Get release URL
if [ "$USE_DEV" = true ]; then
  VERSION="dev"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/dev/${ASSET_NAME}"
else
  VERSION="${CS_VERSION:-latest}"
  if [ "$VERSION" = "latest" ]; then
    DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ASSET_NAME}"
  else
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ASSET_NAME}"
  fi
fi

info "Detected: ${PLATFORM}/${ARCH_NAME}"
info "Downloading: ${DOWNLOAD_URL}"

# Download, verify, and extract
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

curl -fsSL "$DOWNLOAD_URL" -o "${TMP_DIR}/${ASSET_NAME}" || error "Download failed. Check the URL or your network."
CHECKSUM_URL="${DOWNLOAD_URL}.sha256"
CHECKSUM_FILE="${TMP_DIR}/${ASSET_NAME}.sha256"
curl -fsSL "$CHECKSUM_URL" -o "$CHECKSUM_FILE" || error "Checksum download failed. The release is incomplete or your network is unavailable."

EXPECTED_SHA256="$(awk -v filename="$ASSET_NAME" '
  NF != 2 { exit 1 }
  length($1) != 64 || $1 !~ /^[[:xdigit:]]+$/ { exit 1 }
  $2 != filename && $2 != "*" filename { exit 1 }
  NR > 1 { exit 1 }
  { print tolower($1) }
  END { if (NR != 1) exit 1 }
' "$CHECKSUM_FILE")" || error "Invalid checksum file for ${ASSET_NAME}."
[ -n "$EXPECTED_SHA256" ] || error "Checksum file for ${ASSET_NAME} is empty."

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA256="$(sha256sum "${TMP_DIR}/${ASSET_NAME}" | awk '{print tolower($1)}')"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL_SHA256="$(shasum -a 256 "${TMP_DIR}/${ASSET_NAME}" | awk '{print tolower($1)}')"
else
  error "Neither sha256sum nor shasum is available to verify the download."
fi

[ "$ACTUAL_SHA256" = "$EXPECTED_SHA256" ] || error "Checksum mismatch for ${ASSET_NAME}; refusing to extract it."
info "Checksum verified: ${ASSET_NAME}"
tar xzf "${TMP_DIR}/${ASSET_NAME}" -C "$TMP_DIR"

# Install
if [ -w "$INSTALL_DIR" ]; then
  install -m 0755 "${TMP_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
else
  info "Installing to ${INSTALL_DIR} (requires sudo)"
  sudo install -m 0755 "${TMP_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
fi

info "Installed: $(${INSTALL_DIR}/${BINARY_NAME} --version)"
info "Run 'codex-switch --help' to get started"
