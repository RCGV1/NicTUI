#!/usr/bin/env python3
"""Launch the NicTUI debug binary in Terminal.app, send keys, and capture a PNG.

This is intentionally macOS-specific and meant for local UI iteration loops.
"""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT_DIR = ROOT / ".ui-captures"
DEFAULT_BINARY = ROOT / "target" / "debug" / "nictui"

SPECIAL_KEYS = {
    "up": "key code 126",
    "down": "key code 125",
    "left": "key code 123",
    "right": "key code 124",
    "enter": "key code 36",
    "return": "key code 36",
    "esc": "key code 53",
    "escape": "key code 53",
    "tab": "key code 48",
    "space": "key code 49",
    "backtab": "key code 48 using shift down",
}


def run_applescript(script: str) -> str:
    result = subprocess.run(
        ["osascript", "-"],
        input=script,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        error = result.stderr.strip() or "AppleScript execution failed"
        if "-1719" in error:
            raise RuntimeError(
                "System Events does not have Accessibility access. Enable Terminal/osascript under System Settings > Privacy & Security > Accessibility."
            )
        raise RuntimeError(error)
    return result.stdout.strip()


def shell_quote(value: str) -> str:
    return shlex.quote(value)


def applescript_quote(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def launch_terminal(command: str, bounds: tuple[int, int, int, int], title: str) -> None:
    x, y, width, height = bounds
    script = f"""
tell application "Terminal"
    activate
    do script {applescript_quote(command)}
    delay 0.4
    set bounds of front window to {{{x}, {y}, {x + width}, {y + height}}}
end tell
"""
    run_applescript(script)
    time.sleep(0.8)


def terminal_bounds() -> tuple[int, int, int, int]:
    output = run_applescript(
        """
tell application "Terminal"
    activate
    set windowBounds to bounds of front window
    set leftEdge to item 1 of windowBounds
    set topEdge to item 2 of windowBounds
    set rightEdge to item 3 of windowBounds
    set bottomEdge to item 4 of windowBounds
    return (leftEdge as text) & "," & (topEdge as text) & "," & ((rightEdge - leftEdge) as text) & "," & ((bottomEdge - topEdge) as text)
end tell
"""
    )
    x, y, width, height = [int(value.strip()) for value in output.split(",")]
    return x, y, width, height


def send_key(token: str) -> None:
    token = token.strip()
    if not token:
        return
    if token.startswith("wait:"):
        time.sleep(float(token.split(":", 1)[1]))
        return

    command = SPECIAL_KEYS.get(token.lower())
    if command is None:
        if len(token) != 1:
            raise ValueError(
                f"Unsupported key token {token!r}. Use a single character or one of: {', '.join(sorted(SPECIAL_KEYS))}, wait:SECONDS"
            )
        command = f"keystroke {applescript_quote(token)}"

    run_applescript(
        f"""
tell application "System Events"
    tell process "Terminal"
        set frontmost to true
        {command}
    end tell
end tell
"""
    )
    time.sleep(0.25)


def close_terminal_window() -> None:
    run_applescript(
        """
tell application "Terminal"
    if (count of windows) > 0 then
        close front window saving no
    end if
end tell
"""
    )


def build_binary(binary: Path) -> None:
    result = subprocess.run(
        ["cargo", "build"],
        cwd=ROOT,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError("cargo build failed")
    if not binary.exists():
        raise RuntimeError(f"Expected debug binary at {binary}")


def screenshot(bounds: tuple[int, int, int, int], output: Path) -> None:
    x, y, width, height = bounds
    subprocess.run(
        [
            "screencapture",
            "-x",
            "-R",
            f"{x},{y},{width},{height}",
            str(output),
        ],
        check=True,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Launch the NicTUI debug TUI, send keys, and capture a macOS screenshot."
    )
    parser.add_argument(
        "--binary",
        default=str(DEFAULT_BINARY),
        help="Path to the nictui debug binary",
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="Run cargo build before launching the capture session",
    )
    parser.add_argument(
        "--output",
        help="PNG output path. Defaults to .ui-captures/tui-YYYYMMDD-HHMMSS.png",
    )
    parser.add_argument(
        "--keys",
        default="",
        help="Comma-separated key script such as 'r,wait:1.5,down,down'",
    )
    parser.add_argument(
        "--delay",
        type=float,
        default=1.0,
        help="Initial settle time in seconds after launch",
    )
    parser.add_argument(
        "--after-keys-delay",
        type=float,
        default=0.8,
        help="Extra settle time in seconds before taking the screenshot",
    )
    parser.add_argument(
        "--port",
        help="Optional serial port to pass to 'nictui tui --port ...'",
    )
    parser.add_argument(
        "--bounds",
        default="100,80,1100,760",
        help="Terminal window bounds as x,y,width,height",
    )
    parser.add_argument(
        "--keep-window",
        action="store_true",
        help="Leave the Terminal window open after capture",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    binary = Path(args.binary).expanduser().resolve()
    if args.build:
        build_binary(binary)
    if not binary.exists():
        raise SystemExit(f"Binary not found: {binary}")

    bounds = tuple(int(part.strip()) for part in args.bounds.split(","))
    if len(bounds) != 4:
        raise SystemExit("--bounds must be x,y,width,height")

    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    output = (
        Path(args.output).expanduser().resolve()
        if args.output
        else (DEFAULT_OUTPUT_DIR / f"tui-{timestamp}.png").resolve()
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    metadata_path = output.with_suffix(".json")

    command_parts = [
        "cd",
        shell_quote(str(ROOT)),
        "&&",
        "export",
        "TERM=xterm-256color",
        "CLICOLOR_FORCE=1",
        "COLORTERM=truecolor",
        "&&",
        "printf",
        shell_quote(f"\\033]0;NicTUI Capture {timestamp}\\007"),
        "&&",
        "exec",
        shell_quote(str(binary)),
        "tui",
    ]
    if args.port:
        command_parts.extend(["--port", shell_quote(args.port)])
    command = " ".join(command_parts)

    launch_terminal(command, bounds, f"NicTUI Capture {timestamp}")
    time.sleep(args.delay)

    tokens = [token.strip() for token in args.keys.split(",") if token.strip()]
    for token in tokens:
        send_key(token)

    time.sleep(args.after_keys_delay)
    actual_bounds = terminal_bounds()
    screenshot(actual_bounds, output)

    metadata = {
        "timestamp": timestamp,
        "binary": str(binary),
        "command": command,
        "port": args.port,
        "keys": tokens,
        "window_bounds": {
            "x": actual_bounds[0],
            "y": actual_bounds[1],
            "width": actual_bounds[2],
            "height": actual_bounds[3],
        },
        "screenshot": str(output),
    }
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")

    if not args.keep_window:
        close_terminal_window()

    print(output)
    print(metadata_path)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # pragma: no cover - script error path
        print(f"tui_capture_macos.py: {exc}", file=sys.stderr)
        raise SystemExit(1)
