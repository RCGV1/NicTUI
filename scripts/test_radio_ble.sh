#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
BLE_DEVICE="${NICTUI_BLE_DEVICE:-}"
BLE_NAME="${NICTUI_BLE_NAME:-TD-H3}"
TIMEOUT="${NICTUI_BLE_TIMEOUT:-8}"
RUN_REMOTE=1
BUILD_APP=0
OUTPUT_DIR="${ROOT_DIR}/.live-debug/ble-smoke"

usage() {
    cat <<'EOF'
Run a focused BLE smoke test against a real radio.

This helper prefers a fixed BLE device identifier so testing does not depend on
the radio advertising often enough for name-based discovery.

Usage:
  scripts/test_radio_ble.sh [options]

Options:
  --device <id>        Explicit BLE device UUID / identifier to use.
  --name <name>        Fallback BLE name. Defaults to TD-H3.
  --timeout <seconds>  BLE scan/doctor timeout. Defaults to 8.
  --output-dir <dir>   Directory for logs and JSON output.
  --skip-remote        Skip remote diagnose in the smoke test.
  --build-app          Build NicTUI.app first and prefer it on macOS.
  --help               Show this help text.

Environment:
  NICTUI_BLE_DEVICE    Preferred explicit BLE device UUID / identifier.
  NICTUI_BLE_NAME      Fallback BLE name when no explicit device is set.
  NICTUI_BLE_TIMEOUT   Default timeout override in seconds.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --device)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --device" >&2
                exit 1
            fi
            BLE_DEVICE="$2"
            shift 2
            ;;
        --device=*)
            BLE_DEVICE="${1#--device=}"
            shift
            ;;
        --name)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --name" >&2
                exit 1
            fi
            BLE_NAME="$2"
            shift 2
            ;;
        --name=*)
            BLE_NAME="${1#--name=}"
            shift
            ;;
        --timeout)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --timeout" >&2
                exit 1
            fi
            TIMEOUT="$2"
            shift 2
            ;;
        --timeout=*)
            TIMEOUT="${1#--timeout=}"
            shift
            ;;
        --output-dir)
            if [[ $# -lt 2 ]]; then
                echo "Missing value for --output-dir" >&2
                exit 1
            fi
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --output-dir=*)
            OUTPUT_DIR="${1#--output-dir=}"
            shift
            ;;
        --skip-remote)
            RUN_REMOTE=0
            shift
            ;;
        --build-app)
            BUILD_APP=1
            shift
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
done

mkdir -p "${OUTPUT_DIR}"

resolve_nictui() {
    local host_triple app_path debug_bin
    host_triple="$(rustc -vV | awk '/^host:/ {print $2}')"
    app_path="${TARGET_DIR}/macos-app/NicTUI.app/Contents/MacOS/nictui"
    debug_bin="${TARGET_DIR}/debug/nictui"

    if [[ "$(uname -s)" == "Darwin" ]]; then
        if [[ "${BUILD_APP}" -eq 1 ]]; then
            "${ROOT_DIR}/scripts/build_macos_app.sh" \
                --target "${host_triple}" \
                --output-dir "${TARGET_DIR}/macos-app" \
                --no-sign >/dev/null
        fi
        if [[ -x "${app_path}" ]]; then
            printf '%s\n' "${app_path}"
            return 0
        fi
    fi

    if [[ ! -x "${debug_bin}" ]]; then
        (cd "${ROOT_DIR}" && cargo build --bin nictui >/dev/null)
    fi
    printf '%s\n' "${debug_bin}"
}

NICTUI_BIN="$(resolve_nictui)"
LOG_FILE="${OUTPUT_DIR}/ble-smoke.log"
APP_STEP_LOG=""

run_cmd() {
    local label="$1"
    shift
    local rc=0
    echo
    echo "== ${label} =="
    echo "$*"
    {
        echo
        echo "== ${label} =="
        echo "$*"
    } >> "${LOG_FILE}"
    APP_STEP_LOG="${OUTPUT_DIR}/$(echo "${label}" | tr '[:upper:] ' '[:lower:]-' | tr -cd 'a-z0-9-_').app.log"
    rm -f "${APP_STEP_LOG}"
    if [[ "$1" == *"/NicTUI.app/Contents/MacOS/nictui" ]]; then
        "$@" --app-log-file "${APP_STEP_LOG}" 2>&1 | tee -a "${LOG_FILE}" || rc=$?
        if [[ -f "${APP_STEP_LOG}" ]]; then
            echo "--- ${label} app log ---" | tee -a "${LOG_FILE}"
            cat "${APP_STEP_LOG}" | tee -a "${LOG_FILE}"
        fi
    else
        "$@" 2>&1 | tee -a "${LOG_FILE}" || rc=$?
    fi
    return "${rc}"
}

echo "Writing BLE smoke output to ${OUTPUT_DIR}"
echo "Using NicTUI binary: ${NICTUI_BIN}"
if [[ -n "${BLE_DEVICE}" ]]; then
    echo "Using explicit BLE device: ${BLE_DEVICE}"
else
    echo "Using BLE name fallback: ${BLE_NAME}"
    echo "Tip: export NICTUI_BLE_DEVICE=<uuid> to avoid depending on advertisements."
fi

if [[ -n "${BLE_DEVICE}" ]]; then
    TARGET_ARGS=(--ble-device "${BLE_DEVICE}")
    DOCTOR_ARGS=(--device "${BLE_DEVICE}")
else
    TARGET_ARGS=(--ble-name "${BLE_NAME}")
    DOCTOR_ARGS=(--name "${BLE_NAME}")
fi

run_cmd "BLE Doctor" \
    "${NICTUI_BIN}" bluetooth doctor \
    --timeout "${TIMEOUT}" \
    "${DOCTOR_ARGS[@]}"

run_cmd "Probe" \
    "${NICTUI_BIN}" probe \
    "${TARGET_ARGS[@]}"

run_cmd "Channels Read" \
    "${NICTUI_BIN}" channels read \
    --output "${OUTPUT_DIR}/channels.json" \
    "${TARGET_ARGS[@]}"

if [[ "${RUN_REMOTE}" -eq 1 ]]; then
    run_cmd "Remote Diagnose" \
        "${NICTUI_BIN}" remote diagnose \
        --json \
        "${TARGET_ARGS[@]}" \
        | tee "${OUTPUT_DIR}/remote-diagnose.json"
fi

echo
echo "BLE smoke test complete."
echo "Log: ${LOG_FILE}"
