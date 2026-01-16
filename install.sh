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
    VERSION=$(gh api repos/${REPO}/releases/latest --jq '.tag_name' 2>/dev/null | sed 's/^v//')
    if [ -z "$VERSION" ]; then
        echo "Error: Could not fetch latest version" >&2
        exit 1
    fi
    echo "$VERSION"
}

get_download_url() {
    local VERSION="$1"
    echo "https://github.com/${REPO}/releases/download/v${VERSION}/nictui-${VERSION}-${PLATFORM}"
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
            echo "${INSTALL_DIR} is already in PATH"
        fi
    else
        echo "# NicTUI installation" > "$shell_config"
        echo "$path_line" >> "$shell_config"
        echo "Created ${shell_config} with PATH configuration"
    fi
}

is_installed() {
    if [ -f "$INSTALL_PATH" ]; then
        INSTALLED_VERSION=$("$INSTALL_PATH" --version 2>/dev/null || echo "")
        if [ -n "$INSTALLED_VERSION" ]; then
            return 0
        fi
    fi
    return 1
}

get_installed_version() {
    if [ -f "$INSTALL_PATH" ]; then
        "$INSTALL_PATH" --version 2>/dev/null || echo ""
    fi
}

install_nictui() {
    local VERSION="$1"
    local DOWNLOAD_URL
    local TEMP_DIR

    echo "Detected platform: ${PLATFORM}"
    echo "Installing NicTUI v${VERSION}..."

    DOWNLOAD_URL=$(get_download_url "$VERSION")

    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    local ARCHIVE="${TEMP_DIR}/nictui-${PLATFORM}"

    echo "Downloading from ${DOWNLOAD_URL}..."

    if ! curl -fsSL -o "$ARCHIVE" "$DOWNLOAD_URL"; then
        echo "Error: Download failed. The binary for ${PLATFORM} may not be available yet." >&2
        exit 1
    fi

    chmod +x "$ARCHIVE"

    mkdir -p "$INSTALL_DIR"

    if [ -f "$INSTALL_PATH" ]; then
        echo "Backing up existing installation..."
        cp "$INSTALL_PATH" "${INSTALL_PATH}.backup"
    fi

    mv "$ARCHIVE" "$INSTALL_PATH"

    echo "Installed NicTUI to ${INSTALL_PATH}"

    add_to_path

    echo ""
    echo "========================================"
    echo "Installation complete!"
    echo ""
    echo "To start NicTUI, run:"
    echo "  ${INSTALL_PATH}"
    echo ""
    echo "Or add ${INSTALL_DIR} to your PATH and run:"
    echo "  nictui"
    echo ""
    echo "Note: You may need to restart your terminal or run:"
    echo "  source ~/.bashrc  # or ~/.zshrc, ~/.profile, etc."
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

    echo "NicTUI Installer"
    echo "================"

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
        fi
    else
        echo "NicTUI is not installed."
    fi

    install_nictui "$LATEST_VERSION"
}

main "$@"
