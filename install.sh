#!/bin/bash
set -eo pipefail

REPO="RCGV1/NicTUI"
INSTALL_DIR="${HOME}/.local/bin"
MACOS_APP_INSTALL_DIR="${HOME}/Applications"
BINARY_NAME="nictui"
ASSET_EXTENSION=""
INSTALL_PATH="${INSTALL_DIR}/${BINARY_NAME}"
APP_INSTALL_PATH="${MACOS_APP_INSTALL_DIR}/NicTUI.app"
INSTALL_KIND=""

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux*)
            case "$ARCH" in
                x86_64)
                    PLATFORM="linux-x86_64"
                    ;;
                *)
                    echo "Error: Unsupported Linux architecture: $ARCH" >&2
                    echo "Published release assets currently support Linux x86_64 only." >&2
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
            case "$ARCH" in
                x86_64|amd64|AMD64)
                    PLATFORM="windows-x86_64"
                    BINARY_NAME="nictui.exe"
                    ASSET_EXTENSION=".exe"
                    ;;
                *)
                    echo "Error: Unsupported Windows architecture: $ARCH" >&2
                    echo "Published release assets currently support Windows x86_64 only." >&2
                    exit 1
                    ;;
            esac
            ;;
        *)
            echo "Error: Unsupported operating system: $OS" >&2
            exit 1
            ;;
    esac

    INSTALL_PATH="${INSTALL_DIR}/${BINARY_NAME}"
}

get_latest_version() {
    echo "Fetching latest version from GitHub..." >&2
    local VERSION
    VERSION=$(curl --fail -sSL --connect-timeout 5 --max-time 15 "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\([^"]*\)".*/\1/p')

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
    local ASSET_NAME="nictui-${VERSION}-${PLATFORM}${ASSET_EXTENSION}"
    local URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ASSET_NAME}"

    if ! curl --fail -sSL --connect-timeout 10 --max-time 120 -L -o "$OUTPUT_FILE" "$URL"; then
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

download_app_bundle() {
    local VERSION="$1"
    local OUTPUT_FILE="$2"
    local ASSET_NAME="NicTUI-${VERSION}-${PLATFORM}.app.zip"
    local URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ASSET_NAME}"

    echo "Downloading signed and notarized NicTUI.app v${VERSION}..." >&2
    if ! curl --fail -sSL --connect-timeout 10 --max-time 120 -L -o "$OUTPUT_FILE" "$URL"; then
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

verify_asset_checksum() {
    local VERSION="$1"
    local FILE_PATH="$2"
    local TEMP_DIR="$3"
    local ASSET_NAME="$4"
    local CHECKSUMS_FILE="${TEMP_DIR}/checksums.txt"
    local ASSET_CHECKSUM_FILE="${TEMP_DIR}/${ASSET_NAME}.sha256"
    local CHECKSUMS_URL="https://github.com/${REPO}/releases/download/v${VERSION}/checksums.txt"

    echo "Downloading checksums..." >&2
    if ! curl --fail -sSL --connect-timeout 10 --max-time 60 -L -o "$CHECKSUMS_FILE" "$CHECKSUMS_URL"; then
        echo "Error: Could not download checksums.txt" >&2
        return 1
    fi

    if ! awk -v asset="$ASSET_NAME" '$2 == asset { print; found = 1 } END { exit found ? 0 : 1 }' "$CHECKSUMS_FILE" > "$ASSET_CHECKSUM_FILE"; then
        echo "Error: checksums.txt does not contain ${ASSET_NAME}" >&2
        return 1
    fi

    cp "$FILE_PATH" "${TEMP_DIR}/${ASSET_NAME}"
    (
        cd "$TEMP_DIR"
        if command -v sha256sum >/dev/null 2>&1; then
            sha256sum -c "${ASSET_NAME}.sha256"
        elif command -v shasum >/dev/null 2>&1; then
            shasum -a 256 -c "${ASSET_NAME}.sha256"
        else
            echo "Error: sha256sum or shasum is required to verify ${ASSET_NAME}" >&2
            exit 1
        fi
    )
}

verify_binary_checksum() {
    local VERSION="$1"
    local FILE_PATH="$2"
    local TEMP_DIR="$3"
    local ASSET_NAME="nictui-${VERSION}-${PLATFORM}${ASSET_EXTENSION}"

    verify_asset_checksum "$VERSION" "$FILE_PATH" "$TEMP_DIR" "$ASSET_NAME"
}

verify_app_checksum() {
    local VERSION="$1"
    local FILE_PATH="$2"
    local TEMP_DIR="$3"
    local ASSET_NAME="NicTUI-${VERSION}-${PLATFORM}.app.zip"

    verify_asset_checksum "$VERSION" "$FILE_PATH" "$TEMP_DIR" "$ASSET_NAME"
}

add_to_path() {
    local shell_config=""
    local shell_name="$(basename "${SHELL:-bash}")"
    local block_start="# >>> NicTUI installer >>>"
    local block_end="# <<< NicTUI installer <<<"

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
    if [ "$shell_name" = "fish" ]; then
        path_line="set -gx PATH \"${INSTALL_DIR}\" \$PATH"
        mkdir -p "$(dirname "$shell_config")"
    fi
    touch "$shell_config"

    local tmp_file
    tmp_file=$(mktemp)

    awk -v start="$block_start" -v end="$block_end" '
        $0 == start { in_block = 1; next }
        $0 == end { in_block = 0; next }
        !in_block { print }
    ' "$shell_config" > "$tmp_file"

    if cmp -s "$shell_config" "$tmp_file"; then
        rm -f "$tmp_file"
    else
        mv "$tmp_file" "$shell_config"
    fi

    if ! grep -qF "${INSTALL_DIR}" "$shell_config" 2>/dev/null; then
        echo "" >> "$shell_config"
        echo "$block_start" >> "$shell_config"
        echo "# Managed by install.sh. Remove this block to opt out." >> "$shell_config"
        echo "$path_line" >> "$shell_config"
        echo "$block_end" >> "$shell_config"
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
        local parsed_ver
        parsed_ver=$(printf '%s\n' "$ver" | sed -nE 's/.*([0-9]+(\.[0-9]+){2}([-+][0-9A-Za-z.-]+)?).*/\1/p' | head -n 1)
        if [ -n "$parsed_ver" ]; then
            echo "$parsed_ver"
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

    if ! verify_binary_checksum "$VERSION" "$ARCHIVE" "$TEMP_DIR"; then
        echo "Error: Checksum verification failed" >&2
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
    NEW_VER=$(get_installed_version)
    echo "Installed to ${INSTALL_PATH}"
    echo "Binary version: ${NEW_VER}"
}

install_app_bundle() {
    local VERSION="$1"
    local TEMP_DIR
    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT

    local ARCHIVE="${TEMP_DIR}/NicTUI.app.zip"
    local EXTRACT_DIR="${TEMP_DIR}/app"

    if ! download_app_bundle "$VERSION" "$ARCHIVE"; then
        echo "" >&2
        echo "Manual download:"
        echo "  https://github.com/${REPO}/releases/latest"
        echo ""
        echo "For CLI-only scripting, run:"
        echo "  curl -fsSL https://raw.githubusercontent.com/${REPO}/master/install.sh | bash -s -- --cli"
        exit 1
    fi

    if ! verify_app_checksum "$VERSION" "$ARCHIVE" "$TEMP_DIR"; then
        echo "Error: Checksum verification failed" >&2
        exit 1
    fi

    if ! command -v unzip >/dev/null 2>&1; then
        echo "Error: unzip is required to install NicTUI.app" >&2
        exit 1
    fi

    mkdir -p "$EXTRACT_DIR" "$MACOS_APP_INSTALL_DIR"
    if ! unzip -q "$ARCHIVE" -d "$EXTRACT_DIR"; then
        echo "Error: Failed to extract NicTUI.app archive" >&2
        exit 1
    fi

    if [ ! -d "${EXTRACT_DIR}/NicTUI.app" ]; then
        echo "Error: NicTUI.app was not found in the downloaded archive" >&2
        exit 1
    fi

    rm -rf "$APP_INSTALL_PATH"
    if ! cp -R "${EXTRACT_DIR}/NicTUI.app" "$APP_INSTALL_PATH"; then
        echo "Error: Failed to install to ${APP_INSTALL_PATH}" >&2
        exit 1
    fi

    echo "Installed signed and notarized app to ${APP_INSTALL_PATH}"
    echo "Launch with: open \"${APP_INSTALL_PATH}\""
}

main() {
    local arg1="${1:-}"

    detect_platform

    INSTALL_KIND="cli"
    if [ "$(uname -s)" = "Darwin" ]; then
        INSTALL_KIND="app"
    fi

    case "$arg1" in
        --cli)
            INSTALL_KIND="cli"
            arg1=""
            ;;
        --app)
            INSTALL_KIND="app"
            arg1=""
            ;;
    esac

    if [ "$arg1" = "--version" ] || [ "$arg1" = "-v" ]; then
        if is_installed; then
            echo "NicTUI v$(get_installed_version)"
        else
            echo "NicTUI is not installed"
        fi
        exit 0
    fi

    if [ "$arg1" = "--uninstall" ]; then
        if [ "$INSTALL_KIND" = "app" ]; then
            if [ -d "$APP_INSTALL_PATH" ]; then
                rm -rf "$APP_INSTALL_PATH"
                echo "NicTUI.app has been uninstalled"
            else
                echo "NicTUI.app is not installed"
            fi
            exit 0
        fi

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
        echo "  --app          Install the signed/notarized macOS app bundle"
        echo "  --cli          Install the raw CLI binary for scripting"
        echo "  --help, -h     Show this help"
        echo ""
        echo "Default behavior:"
        echo "  macOS installs NicTUI.app for the recommended Bluetooth permission UX."
        echo "  Linux, Windows, and --cli install the raw CLI binary to ${INSTALL_DIR}."
        echo ""
        echo "Manual download:"
        echo "  https://github.com/${REPO}/releases/latest"
        exit 0
    fi

    if [ "$INSTALL_KIND" = "app" ] && [ "$(uname -s)" != "Darwin" ]; then
        echo "Error: --app is only available on macOS." >&2
        exit 1
    fi

    echo "NicTUI Installer"
    echo "================"
    if [ "$INSTALL_KIND" = "app" ]; then
        echo "macOS default: installing the signed/notarized app bundle."
        echo "Use --cli only when you explicitly need the raw command-line binary."
        echo ""
    fi

    local LATEST_VERSION
    LATEST_VERSION=$(get_latest_version)

    if [ "$INSTALL_KIND" = "app" ]; then
        install_app_bundle "$LATEST_VERSION"
        echo ""
        echo "========================================"
        echo "Installation complete!"
        echo ""
        echo "Run:"
        echo "  open \"${APP_INSTALL_PATH}\""
        echo ""
        echo "CLI-only scripting install:"
        echo "  curl -fsSL https://raw.githubusercontent.com/${REPO}/master/install.sh | bash -s -- --cli"
        echo "========================================"
        exit 0
    fi

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
