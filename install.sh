#!/bin/bash
set -euo pipefail

REPO="RCGV1/NicTUI"
INSTALL_DIR="${HOME}/.local/bin"
BINARY_NAME="nictui"
INSTALL_PATH="${INSTALL_DIR}/${BINARY_NAME}"

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux*)
            case "$ARCH" in
                x86_64)
                    PLATFORM="linux-x86_64"
                    ;;
                aarch64|arm64)
                    PLATFORM="linux-aarch64"
                    ;;
                *)
                    echo "Error: Unsupported architecture: $ARCH" >&2
                    exit 1
                    ;;
            esac
            ;;
        Darwin*)
            case "$ARCH" in
                x86_64)
                    PLATFORM="macos-x86_64"
                    ;;
                arm64|aarch64)
                    PLATFORM="macos-aarch64"
                    ;;
                *)
                    echo "Error: Unsupported architecture: $ARCH" >&2
                    exit 1
                    ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            PLATFORM="windows-x86_64"
            ;;
        *)
            echo "Error: Unsupported operating system: $OS" >&2
            exit 1
            ;;
    esac
}

get_latest_version() {
    local VERSION=""

    echo "Fetching latest version from GitHub..."
    VERSION=$(curl -sSL --connect-timeout 5 --max-time 15 "https://api.github.com/repos/${REPO}/releases/latest" 2>&1 | grep '"tag_name"' | sed 's/.*"v\([0-9.]*\)".*/\1/')

    if [ -z "$VERSION" ]; then
        echo "Error: Could not fetch latest version." >&2
        echo "Check your internet connection." >&2
        echo "" >&2
        echo "Manual download:" >&2
        echo "  https://github.com/${REPO}/releases/latest" >&2
        exit 1
    fi
    echo "Latest version: v${VERSION}"
    echo "$VERSION"
}

download_with_fallback() {
    local VERSION="$1"
    local OUTPUT_FILE="$2"
    local DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/nictui-${VERSION}-${PLATFORM}"

    echo "Downloading NicTUI v${VERSION}..."
    curl -sSL --connect-timeout 10 --max-time 120 -L -o "$OUTPUT_FILE" "$DOWNLOAD_URL"

    if [ -s "$OUTPUT_FILE" ]; then
        echo "Download complete!"
        return 0
    fi

    echo "Error: Download failed" >&2
    return 1
}

add_to_path() {
    local shell_config=""
    local shell_name="$(basename "${SHELL:-bash}")"

    case "$shell_name" in
        bash)
            shell_config="${HOME}/.bashrc"
            ;;
        zsh)
            shell_config="${HOME}/.zshrc"
            ;;
        fish)
            shell_config="${HOME}/.config/fish/config.fish"
            ;;
        *)
            shell_config="${HOME}/.profile"
            ;;
    esac

    local path_line="export PATH=\"${INSTALL_DIR}:\$PATH\""

    if [ -f "$shell_config" ]; then
        if ! grep -qF "${INSTALL_DIR}" "$shell_config" 2>/dev/null; then
            echo "" >> "$shell_config"
            echo "# NicTUI installation" >> "$shell_config"
            echo "$path_line" >> "$shell_config"
            echo "Added ${INSTALL_DIR} to PATH"
        fi
    fi
}

is_installed() {
    if [ -f "$INSTALL_PATH" ]; then
        return 0
    fi
    return 1
}

get_installed_version() {
    if [ -f "$INSTALL_PATH" ] && [ -x "$INSTALL_PATH" ]; then
        local ver=$("$INSTALL_PATH" --version 2>/dev/null || echo "")
        if [ -n "$ver" ]; then
            echo "$ver"
        else
            echo "unknown"
        fi
    fi
}

install_nictui() {
    local VERSION="$1"
    local IS_UPDATE="$2"
    local TEMP_DIR

    if [ "$IS_UPDATE" = "true" ]; then
        echo "Updating NicTUI to v${VERSION}..."
    else
        echo "Installing NicTUI v${VERSION}..."
    fi

    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    local ARCHIVE="${TEMP_DIR}/nictui-${PLATFORM}"

    if ! download_with_fallback "$VERSION" "$ARCHIVE"; then
        exit 1
    fi

    chmod +x "$ARCHIVE"

    mkdir -p "$INSTALL_DIR"

    if ! cp -f "$ARCHIVE" "$INSTALL_PATH"; then
        echo "Error: Failed to copy to ${INSTALL_PATH}" >&2
        echo "Try: chmod u+w ${INSTALL_DIR}" >&2
        exit 1
    fi

    chmod +x "$INSTALL_PATH"

    echo "Installed to ${INSTALL_PATH}"

    local NEW_VERSION=$("$INSTALL_PATH" --version 2>/dev/null || echo "unknown")
    echo "Binary version: ${NEW_VERSION}"

    add_to_path

    echo ""
    echo "========================================"
    echo "Installation complete!"
    echo ""
    echo "To use immediately, run:"
    echo "  hash -r && ${INSTALL_PATH}"
    echo ""
    echo "Or in new terminals:"
    echo "  nictui"
    echo "========================================"
}

main() {
    detect_platform

    if [ "$1" = "--version" ] || [ "$1" = "-v" ]; then
        if is_installed; then
            echo "NicTUI v$(get_installed_version)"
        else
            echo "NicTUI is not installed"
        fi
        exit 0
    fi

    if [ "$1" = "--uninstall" ]; then
        if [ -f "$INSTALL_PATH" ]; then
            rm -f "$INSTALL_PATH"
            echo "NicTUI has been uninstalled"
        else
            echo "NicTUI is not installed"
        fi
        exit 0
    fi

    if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
        echo "NicTUI Installer"
        echo ""
        echo "Usage: $0 [OPTIONS]"
        echo ""
        echo "Options:"
        echo "  --version, -v  Show installed version"
        echo "  --uninstall    Remove NicTUI"
        echo "  --help, -h     Show this help"
        echo ""
        echo "For manual installation, visit:"
        echo "  https://github.com/${REPO}/releases/latest"
        exit 0
    fi

    echo "NicTUI Installer"
    echo "================"

    LATEST_VERSION=$(get_latest_version)

    if is_installed; then
        INSTALLED_VERSION=$(get_installed_version)
        echo "Installed version: ${INSTALLED_VERSION}"

        if [ "$INSTALLED_VERSION" = "$LATEST_VERSION" ]; then
            echo ""
            echo "NicTUI v${LATEST_VERSION} is already installed."
            exit 0
        fi

        echo ""
        echo "Updating to v${LATEST_VERSION}..."
        install_nictui "$LATEST_VERSION" "true"
    else
        echo "NicTUI is not installed."
        echo ""
        install_nictui "$LATEST_VERSION" "false"
    fi
}

main "$@"
