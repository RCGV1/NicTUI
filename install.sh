#!/bin/bash
set -e

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

    VERSION=$(curl -fsSL --connect-timeout 10 --max-time 30 "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null | grep '"tag_name"' | sed 's/.*"v\([0-9.]*\)".*/\1/')

    if [ -z "$VERSION" ]; then
        echo "Error: Could not fetch latest version. Check your internet connection." >&2
        echo "If GitHub is blocked, try downloading manually from:" >&2
        echo "  https://github.com/${REPO}/releases/latest" >&2
        exit 1
    fi
    echo "$VERSION"
}

download_with_fallback() {
    local VERSION="$1"
    local OUTPUT_FILE="$2"
    local DOWNLOAD_URLS=(
        "https://github.com/${REPO}/releases/download/v${VERSION}/nictui-${VERSION}-${PLATFORM}"
        "https://objects.githubusercontent.com/github-production-release-asset-2e65be/325060/$(echo $VERSION | sed 's/\.//g')?${PLATFORM}"
    )

    for url in "${DOWNLOAD_URLS[@]}"; do
        echo "Downloading from GitHub..."
        if curl -fsSL --connect-timeout 10 --max-time 120 -L -o "$OUTPUT_FILE" "$url" 2>/dev/null; then
            if [ -s "$OUTPUT_FILE" ]; then
                echo "Download successful!"
                return 0
            fi
        fi
        echo "Retry..."
    done

    echo "Error: Download failed from all sources" >&2
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
            echo "Added ${INSTALL_DIR} to PATH in ${shell_config}"
        else
            echo "${INSTALL_DIR} is already in your PATH"
        fi
    else
        echo "# NicTUI installation" > "$shell_config"
        echo "$path_line" >> "$shell_config"
        echo "Added ${INSTALL_DIR} to PATH (created ${shell_config})"
    fi
}

is_installed() {
    if [ -f "$INSTALL_PATH" ]; then
        return 0
    fi
    return 1
}

get_installed_version() {
    if [ -f "$INSTALL_PATH" ]; then
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
        echo "Detected platform: ${PLATFORM}"
        echo "Updating NicTUI to v${VERSION}..."
    else
        echo "Detected platform: ${PLATFORM}"
        echo "Installing NicTUI v${VERSION}..."
    fi

    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    local ARCHIVE="${TEMP_DIR}/nictui-${PLATFORM}"

    if ! download_with_fallback "$VERSION" "$ARCHIVE"; then
        echo ""
        echo "Manual download instructions:"
        echo "1. Go to https://github.com/${REPO}/releases/latest"
        echo "2. Download: nictui-${VERSION}-${PLATFORM}"
        echo "3. Save it to ${INSTALL_DIR}/nictui"
        echo "4. Run: chmod +x ${INSTALL_DIR}/nictui"
        exit 1
    fi

    chmod +x "$ARCHIVE"

    mkdir -p "$INSTALL_DIR"

    rm -f "${INSTALL_PATH}.backup"

    if ! cp "$ARCHIVE" "$INSTALL_PATH"; then
        echo "Error: Failed to copy NicTUI to ${INSTALL_PATH}" >&2
        echo "Check permissions: ls -la ${INSTALL_DIR}" >&2
        exit 1
    fi

    if [ ! -x "$INSTALL_PATH" ]; then
        chmod +x "$INSTALL_PATH"
    fi

    local NEW_VERSION=$("$INSTALL_PATH" --version 2>/dev/null || echo "unknown")
    echo "Installed NicTUI to ${INSTALL_PATH}"
    echo "Binary version: ${NEW_VERSION}"

    add_to_path

    echo ""
    echo "========================================"
    echo "Installation complete!"
    echo ""
    echo "NicTUI v${VERSION} has been installed."
    echo ""
    echo "IMPORTANT: Run this command to clear cached paths:"
    echo "  hash -r"
    echo ""
    echo "Then start NicTUI with:"
    echo "  ${INSTALL_PATH}"
    echo ""
    echo "Or from any new terminal:"
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
    echo "Repository: ${REPO}"
    echo "Platform: ${PLATFORM}"

    LATEST_VERSION=$(get_latest_version)
    echo "Latest version: ${LATEST_VERSION}"

    if is_installed; then
        INSTALLED_VERSION=$(get_installed_version)
        echo "Installed version: ${INSTALLED_VERSION}"

        if [ "$INSTALLED_VERSION" = "$LATEST_VERSION" ]; then
            echo ""
            echo "NicTUI v${LATEST_VERSION} is already installed and up to date."
            exit 0
        else
            echo ""
            echo "Updating from v${INSTALLED_VERSION} to v${LATEST_VERSION}..."
            install_nictui "$LATEST_VERSION" "true"
            exit 0
        fi
    else
        echo "NicTUI is not installed."
        echo ""
        install_nictui "$LATEST_VERSION" "false"
    fi
}

main "$@"
