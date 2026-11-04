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

# A terminal invocation already has a screen. Do not send its UI to the log.
if [[ -t 0 && -t 1 ]]; then
    if [[ ${#ARGS[@]} -gt 0 ]]; then
        exec "$APP_BINARY" "${ARGS[@]}"
    else
        exec "$APP_BINARY"
    fi
fi

# Finder provides no terminal. Open a private command file in Terminal.app.
if [[ ${#ARGS[@]} -eq 0 ]] || [[ "${ARGS[0]}" == "tui" ]]; then
    SESSION_DIR="$(mktemp -d "${TMPDIR:-/tmp}/nictui-session.XXXXXX")"
    COMMAND_FILE="${SESSION_DIR}/NicTUI.command"
    {
        printf '#!/bin/bash\n'
        printf 'trap '\''rm -f -- "$0"; rmdir -- "$(dirname "$0")"'\'' EXIT\n'
        printf '%q ' "$APP_BINARY"
        if [[ ${#ARGS[@]} -gt 0 ]]; then
            printf '%q ' "${ARGS[@]}"
        fi
        printf '\n'
    } > "$COMMAND_FILE"
    chmod +x "$COMMAND_FILE"
    if ! open -a Terminal "$COMMAND_FILE"; then
        rm -f -- "$COMMAND_FILE"
        rmdir -- "$SESSION_DIR"
        exit 1
    fi
    exit 0
fi

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
