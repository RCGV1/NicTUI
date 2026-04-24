#!/bin/bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This helper only runs on macOS." >&2
    exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="debug"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"

usage() {
    cat <<'EOF'
Build a local NicTUI.app bundle for macOS BLE/TCC validation.

Usage:
  scripts/build_macos_app.sh [--release] [--target <triple>] [--output-dir <dir>] [--zip <path>] [--no-sign]
                            [--sign-identity <identity>] [--notarize]
                            [--notary-key <path>] [--notary-key-id <id>] [--notary-issuer <uuid>]

Options:
  --release           Build the release binary instead of debug.
  --target <triple>   Cargo target triple to build, for example aarch64-apple-darwin.
  --output-dir <dir>  Directory that will contain NicTUI.app. Defaults to target/macos-app.
  --zip <path>        Also archive NicTUI.app to the given zip file path.
  --no-sign                 Skip ad-hoc signing of the generated app bundle.
  --sign-identity <identity>
                            Sign with a Developer ID Application identity instead of ad-hoc.
                            Can also be set with NICTUI_MACOS_SIGN_IDENTITY.
  --notarize                Submit the app zip to Apple's notary service, staple the app,
                            and recreate the zip. Requires --zip and Developer ID signing.
  --notary-key <path>       App Store Connect API .p8 key path. Env: NICTUI_NOTARY_KEY.
  --notary-key-id <id>      App Store Connect API key ID. Env: NICTUI_NOTARY_KEY_ID.
  --notary-issuer <uuid>    App Store Connect API issuer ID. Env: NICTUI_NOTARY_ISSUER.
  --help                    Show this help text.
EOF
}

SIGN_BUNDLE=1
SIGN_IDENTITY="${NICTUI_MACOS_SIGN_IDENTITY:-}"
NOTARIZE=0
NOTARY_KEY="${NICTUI_NOTARY_KEY:-}"
NOTARY_KEY_ID="${NICTUI_NOTARY_KEY_ID:-}"
NOTARY_ISSUER="${NICTUI_NOTARY_ISSUER:-}"
TARGET_TRIPLE=""
OUTPUT_DIR="${TARGET_DIR}/macos-app"
ZIP_PATH=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            PROFILE="release"
            ;;
        --target)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --target" >&2
                exit 1
            fi
            TARGET_TRIPLE="$2"
            shift
            ;;
        --target=*)
            TARGET_TRIPLE="${1#--target=}"
            ;;
        --output-dir)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --output-dir" >&2
                exit 1
            fi
            OUTPUT_DIR="$2"
            shift
            ;;
        --output-dir=*)
            OUTPUT_DIR="${1#--output-dir=}"
            ;;
        --zip)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --zip" >&2
                exit 1
            fi
            ZIP_PATH="$2"
            shift
            ;;
        --zip=*)
            ZIP_PATH="${1#--zip=}"
            ;;
        --no-sign)
            SIGN_BUNDLE=0
            ;;
        --sign-identity)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --sign-identity" >&2
                exit 1
            fi
            SIGN_IDENTITY="$2"
            shift
            ;;
        --sign-identity=*)
            SIGN_IDENTITY="${1#--sign-identity=}"
            ;;
        --notarize)
            NOTARIZE=1
            ;;
        --notary-key)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --notary-key" >&2
                exit 1
            fi
            NOTARY_KEY="$2"
            shift
            ;;
        --notary-key=*)
            NOTARY_KEY="${1#--notary-key=}"
            ;;
        --notary-key-id)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --notary-key-id" >&2
                exit 1
            fi
            NOTARY_KEY_ID="$2"
            shift
            ;;
        --notary-key-id=*)
            NOTARY_KEY_ID="${1#--notary-key-id=}"
            ;;
        --notary-issuer)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --notary-issuer" >&2
                exit 1
            fi
            NOTARY_ISSUER="$2"
            shift
            ;;
        --notary-issuer=*)
            NOTARY_ISSUER="${1#--notary-issuer=}"
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
    shift
done

if [[ -n "${SIGN_IDENTITY}" && "${SIGN_BUNDLE}" -eq 0 ]]; then
    echo "--no-sign cannot be combined with --sign-identity" >&2
    exit 1
fi
if [[ "${NOTARIZE}" -eq 1 ]]; then
    if [[ -z "${ZIP_PATH}" ]]; then
        echo "--notarize requires --zip so the submitted archive can be recreated after stapling" >&2
        exit 1
    fi
    if [[ -z "${SIGN_IDENTITY}" ]]; then
        echo "--notarize requires --sign-identity with a Developer ID Application certificate" >&2
        exit 1
    fi
    if [[ -z "${NOTARY_KEY}" || -z "${NOTARY_KEY_ID}" || -z "${NOTARY_ISSUER}" ]]; then
        echo "--notarize requires --notary-key, --notary-key-id, and --notary-issuer" >&2
        exit 1
    fi
fi

create_app_zip() {
    local zip_path="$1"
    mkdir -p "$(dirname "${zip_path}")"
    rm -f "${zip_path}"
    COPYFILE_DISABLE=1 ditto -c -k --keepParent --norsrc "${APP_DIR}" "${zip_path}"
}

VERSION="$(
    awk -F'"' '/^version = / { print $2; exit }' "${ROOT_DIR}/Cargo.toml"
)"
if [[ -z "${VERSION}" ]]; then
    echo "Failed to determine package version from Cargo.toml" >&2
    exit 1
fi

echo "Building nictui (${PROFILE})..."
pushd "${ROOT_DIR}" >/dev/null
BUILD_CMD=(cargo build --bin nictui)
if [[ "${PROFILE}" == "release" ]]; then
    BUILD_CMD+=(--release)
fi
if [[ -n "${TARGET_TRIPLE}" ]]; then
    BUILD_CMD+=(--target "${TARGET_TRIPLE}")
fi
"${BUILD_CMD[@]}"
popd >/dev/null

if [[ -n "${TARGET_TRIPLE}" ]]; then
    BINARY_PATH="${TARGET_DIR}/${TARGET_TRIPLE}/${PROFILE}/nictui"
else
    BINARY_PATH="${TARGET_DIR}/${PROFILE}/nictui"
fi
if [[ ! -x "${BINARY_PATH}" ]]; then
    echo "Built binary not found at ${BINARY_PATH}" >&2
    exit 1
fi

APP_DIR="${OUTPUT_DIR}/NicTUI.app"
CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"
PLIST_TEMPLATE="${ROOT_DIR}/macos/Info.plist"
PLIST_PATH="${CONTENTS_DIR}/Info.plist"
WRAPPER_PATH="${MACOS_DIR}/nictui"
APP_BINARY_PATH="${MACOS_DIR}/nictui-bin"

rm -rf "${APP_DIR}"
mkdir -p "${MACOS_DIR}" "${RESOURCES_DIR}"

cp "${BINARY_PATH}" "${APP_BINARY_PATH}"
chmod +x "${APP_BINARY_PATH}"

cat > "${WRAPPER_PATH}" <<'EOF'
#!/bin/bash
set -euo pipefail

SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_BINARY="${SELF_DIR}/nictui-bin"
LOG_FILE="${NICTUI_APP_LOG_FILE:-$HOME/Library/Logs/NicTUI/nictui-app.log}"
ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --app-log-file)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --app-log-file" >&2
                exit 1
            fi
            LOG_FILE="$2"
            shift 2
            ;;
        --app-log-file=*)
            LOG_FILE="${1#--app-log-file=}"
            shift
            ;;
        *)
            ARGS+=("$1")
            shift
            ;;
    esac
done

mkdir -p "$(dirname "$LOG_FILE")"
touch "$LOG_FILE"

{
    echo "===== $(date '+%Y-%m-%d %H:%M:%S') NicTUI.app launch ====="
    echo "Log file: $LOG_FILE"
    if [[ ${#ARGS[@]} -gt 0 ]]; then
        echo "Args: ${ARGS[*]}"
    else
        echo "Args: <none>"
    fi
} >> "$LOG_FILE"

exec >>"$LOG_FILE" 2>&1
if [[ ${#ARGS[@]} -gt 0 ]]; then
    exec "$APP_BINARY" "${ARGS[@]}"
else
    exec "$APP_BINARY"
fi
EOF
chmod +x "${WRAPPER_PATH}"

sed "s/@VERSION@/${VERSION}/g" "${PLIST_TEMPLATE}" > "${PLIST_PATH}"
plutil -lint "${PLIST_PATH}" >/dev/null
xattr -cr "${APP_DIR}"

if [[ -n "${SIGN_IDENTITY}" ]]; then
    echo "Developer ID signing NicTUI.app..."
    codesign --force --timestamp --options runtime --sign "${SIGN_IDENTITY}" "${APP_BINARY_PATH}"
    codesign --force --timestamp --options runtime --sign "${SIGN_IDENTITY}" "${APP_DIR}"
    codesign --verify --deep --strict --verbose=2 "${APP_DIR}"
elif [[ "${SIGN_BUNDLE}" -eq 1 ]]; then
    echo "Ad-hoc signing NicTUI.app..."
    codesign --force --deep --sign - "${APP_DIR}"
fi

if [[ -n "${ZIP_PATH}" ]]; then
    create_app_zip "${ZIP_PATH}"
fi

if [[ "${NOTARIZE}" -eq 1 ]]; then
    echo "Submitting ${ZIP_PATH} for Apple notarization..."
    xcrun notarytool submit "${ZIP_PATH}" \
        --key "${NOTARY_KEY}" \
        --key-id "${NOTARY_KEY_ID}" \
        --issuer "${NOTARY_ISSUER}" \
        --wait
    echo "Stapling notarization ticket to NicTUI.app..."
    xcrun stapler staple "${APP_DIR}"
    xcrun stapler validate "${APP_DIR}"
    create_app_zip "${ZIP_PATH}"
fi

echo "Created ${APP_DIR}"
echo "Launcher: ${WRAPPER_PATH}"
echo "Binary: ${APP_BINARY_PATH}"
echo "Info.plist: ${PLIST_PATH}"
if [[ -n "${TARGET_TRIPLE}" ]]; then
    echo "Target: ${TARGET_TRIPLE}"
fi
echo "Default app log: ${HOME}/Library/Logs/NicTUI/nictui-app.log"
echo "Override at launch with: open ${APP_DIR} --args --app-log-file /path/to/nictui.log ..."
echo "Bluetooth/TCC validation note: launch NicTUI.app outside hosted wrappers such as Codex when testing permission prompts, or macOS may attribute Bluetooth access to the host app instead of NicTUI."
if [[ -n "${SIGN_IDENTITY}" ]]; then
    echo "Signing: Developer ID (${SIGN_IDENTITY})"
elif [[ "${SIGN_BUNDLE}" -eq 1 ]]; then
    echo "Signing: ad-hoc"
else
    echo "Signing: skipped"
fi
if [[ "${NOTARIZE}" -eq 1 ]]; then
    echo "Notarization: stapled"
else
    echo "Notarization: skipped"
fi
if [[ -n "${ZIP_PATH}" ]]; then
    echo "Archive: ${ZIP_PATH}"
fi
