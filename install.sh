#!/bin/bash
set -eo pipefail

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
    echo "Fetching latest version from GitHub..." >&2
    local VERSION
    VERSION=$(curl -sSL --connect-timeout 5 --max-time 15 "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"v\([0-9.]*\)".*/\1/')

    if [ -z "$VERSION" ]; then
        echo "Error: Could not fetch latest version." >&2
        echo "Check your internet connection." >&2
        echo "" >&2
        echo "Manual download:" >&2
        echo "  https://github.com/${REPO}/releases/latest" >&2
        exit 1
    fi
    echo "Latest version: v${VERSION}" >&2
    echo "$VERSION"
}

download_binary() {
    local VERSION="$1"
    local OUTPUT_FILE="$2"

    echo "Downloading NicTUI v${VERSION}..." >&2
    local URL="https://github.com/${REPO}/releases/download/v${VERSION}/nictui-${VERSION}-${PLATFORM}"

    if ! curl -sSL --connect-timeout 10 --max-time 120 -L -o "$OUTPUT_FILE" "$URL"; then
        echo "Error: Download failed" >&2
        return 1
    fi

    if [ ! -s "$OUTPUT_FILE" ]; then
        echo "Error: Downloaded file is empty" >&2
        return 1
    fi

    echo "Download complete!" >&2
    return 0
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

    # Remove any existing NicTUI PATH lines first to avoid duplicates
    if [ -f "$shell_config" ]; then
        local tmp_file
        tmp_file=$(mktemp)
        grep -v "NicTUI installation" "$shell_config" > "$tmp_file" 2>/dev/null || true
        grep -v "export PATH.*\.local/bin.*PATH" "$tmp_file" > "$tmp_file.tmp" 2>/dev/null || true
        mv "$tmp_file.tmp" "$shell_config" 2>/dev/null || cp "$tmp_file" "$shell_config"
        rm -f "$tmp_file"

        # Add the PATH line
        if ! grep -qF "${INSTALL_DIR}" "$shell_config" 2>/dev/null; then
            echo "" >> "$shell_config"
            echo "# NicTUI installation" >> "$shell_config"
            echo "$path_line" >> "$shell_config"
        fi
    fi
}

is_installed() {
    if [ -f "$INSTALL_PATH" ] && [ -x "$INSTALL_PATH" ]; then
        return 0
    fi
    return 1
}

get_installed_version() {
    if [ -f "$INSTALL_PATH" ] && [ -x "$INSTALL_PATH" ]; then
        local ver
        ver=$("$INSTALL_PATH" --version 2>/dev/null || echo "")
        if [ -n "$ver" ]; then
            echo "$ver"
        else
            echo "unknown"
        fi
    fi
}

install_binary() {
    local VERSION="$1"
    local TEMP_DIR
    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    local ARCHIVE="${TEMP_DIR}/nictui"

    if ! download_binary "$VERSION" "$ARCHIVE"; then
        echo "" >&2
        echo "Manual download:" >&2
        echo "  https://github.com/${REPO}/releases/latest" >&2
        exit 1
    fi

    chmod +x "$ARCHIVE"

    mkdir -p "$INSTALL_DIR"

    if ! cp -f "$ARCHIVE" "$INSTALL_PATH"; then
        echo "Error: Failed to install to ${INSTALL_PATH}" >&2
        echo "Check permissions: ls -la ${INSTALL_DIR}" >&2
        exit 1
    fi

    chmod +x "$INSTALL_PATH"

    local NEW_VER
    NEW_VER=$("$INSTALL_PATH" --version 2>/dev/null || echo "unknown")
    echo "Installed to ${INSTALL_PATH}"
    echo "Binary version: ${NEW_VER}"
}

main() {
    local arg1="${1:-}"

    detect_platform

    if [ "$arg1" = "--version" ] || [ "$arg1" = "-v" ]; then
        if is_installed; then
            echo "NicTUI v$(get_installed_version)"
        else
            echo "NicTUI is not installed"
        fi
        exit 0
    fi

    if [ "$arg1" = "--uninstall" ]; then
        if [ -f "$INSTALL_PATH" ]; then
            rm -f "$INSTALL_PATH"
            echo "NicTUI has been uninstalled"
        else
            echo "NicTUI is not installed"
        fi
        exit 0
    fi

    if [ "$arg1" = "--help" ] || [ "$arg1" = "-h" ]; then
        echo "NicTUI Installer"
        echo ""
        echo "Usage: $0 [OPTIONS]"
        echo ""
        echo "Options:"
        echo "  --version, -v  Show installed version"
        echo "  --uninstall    Remove NicTUI"
        echo "  --help, -h     Show this help"
        echo ""
        echo "Manual download:"
        echo "  https://github.com/${REPO}/releases/latest"
        exit 0
    fi

    echo "NicTUI Installer"
    echo "================"

    local LATEST_VERSION
    LATEST_VERSION=$(get_latest_version)

    if is_installed; then
        local INSTALLED_VERSION
        INSTALLED_VERSION=$(get_installed_version)
        echo "Installed version: ${INSTALLED_VERSION}"

        if [ "$INSTALLED_VERSION" = "$LATEST_VERSION" ]; then
            echo ""
            echo "NicTUI v${LATEST_VERSION} is already installed."
            exit 0
        fi

        echo ""
        echo "Updating to v${LATEST_VERSION}..."
        install_binary "$LATEST_VERSION"
    else
        echo "NicTUI is not installed."
        echo ""
        install_binary "$LATEST_VERSION"
    fi

    add_to_path

    echo ""
    echo "========================================"
    echo "Installation complete!"
    echo ""
    echo "IMPORTANT: Clear shell cache with:"
    echo "  hash -r"
    echo ""
    echo "Then run:"
    echo "  ${INSTALL_PATH}"
    echo ""
    echo "Or in new terminals:"
    echo "  nictui"
    echo "========================================"
}

main "$@"
