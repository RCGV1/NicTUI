#!/usr/bin/env python3
"""Hardware-free POSIX terminal and bootloader regression checks.

Run after cargo build: python3 scripts/test_terminal_and_flash.py target/debug/nictui
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time


def read_for(fd, seconds):
    result = b""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if select.select([fd], [], [], max(0, deadline - time.monotonic()))[0]:
            try:
                result += os.read(fd, 65536)
            except OSError:
                break
    return result


def read_exact(fd, size, seconds=3):
    result = b""
    deadline = time.monotonic() + seconds
    while len(result) < size and time.monotonic() < deadline:
        if select.select([fd], [], [], max(0, deadline - time.monotonic()))[0]:
            result += os.read(fd, size - len(result))
    assert len(result) == size, (size, result.hex())
    return result


def terminal_checks(binary):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-ghostty"
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
        os.execv(binary, [binary])
    try:
        output = read_for(fd, 2)
        assert b"NicTUI" in output, "Startup did not draw the port picker"
        assert b"\x1b[?1003h" not in output, "Mouse reporting was enabled"
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        os.kill(pid, signal.SIGWINCH)
        assert read_for(fd, 1), "Resize did not redraw without a keypress"
        os.write(fd, b"q")
        output = read_for(fd, 1)
        assert b"\x1b[?1049l" in output, "Alternate screen was not restored"
        assert b"\x1b[?1006l" in output, "Mouse reporting was not reset"
        assert termios.tcgetattr(fd)[3] & termios.ICANON, "Raw mode was not restored"
        assert os.waitpid(pid, 0)[1] == 0, "TUI exit failed"
        pid = None
    finally:
        if pid is not None:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        os.close(fd)


def flash_checks(binary, size, ack_init=True):
    master, slave = pty.openpty()
    with tempfile.TemporaryDirectory() as directory:
        image = bytes(range(size))
        path = Path(directory) / "firmware.bin"
        path.write_bytes(image)
        process = subprocess.Popen(
            [binary, "firmware", "flash", "--port", os.ttyname(slave), "--input", str(path)],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        try:
            # The serial port is opened after the CLI prints this line.
            while True:
                line = process.stdout.readline()
                assert line, "Flasher exited before opening the simulated port"
                if b"power it on in flash mode" in line:
                    break
            os.write(master, b"\xa5" * 3)
            assert read_exact(master, 36)[0] == 0xA0
            if ack_init:
                os.write(master, b"\xa3")
            packet = read_exact(master, 36)
            assert packet[0] == 0xA1 and packet[1:3] == b"\x00\x00"
            # Drop the first ACK. The same block must be retransmitted.
            assert read_exact(master, 36) == packet, "Lost ACK skipped a firmware block"
            os.write(master, b"\xa3")
            block_count = (size + 31) // 32
            for block in range(1, block_count + 1):
                packet = read_exact(master, 36)
                assert int.from_bytes(packet[1:3], "big") == block
                assert packet[0] == (0xA2 if block == block_count else 0xA1)
                expected = image[block * 32:(block + 1) * 32].ljust(32, b"\x00")
                assert packet[4:] == expected
                assert packet[3] == sum(expected) % 256
                if block != block_count:
                    os.write(master, b"\xa3")
            output = process.communicate(timeout=3)[0]
            assert process.returncode == 0, output.decode(errors="replace")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)


def launcher_checks():
    # Mock only the native window opener; execute the generated command for real.
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        app = root / "App with spaces.app" / "Contents" / "MacOS"
        app.mkdir(parents=True)
        launcher = app / "nictui"
        shutil.copyfile(Path(__file__).resolve().parents[1] / "macos/launcher.sh", launcher)
        launcher.chmod(0o755)
        binary = app / "nictui-bin"
        binary.write_text("#!/usr/bin/env python3\nimport json, os, sys\n"
                          "open(os.environ['NICTUI_TEST_ARGS'], 'w').write(json.dumps(sys.argv[1:]))\n")
        binary.chmod(0o755)
        mock_bin = root / "bin"
        mock_bin.mkdir()
        opener = mock_bin / "open"
        opener.write_text('#!/bin/sh\nprintf "%s" "$3" > "$NICTUI_TEST_COMMAND"\n')
        opener.chmod(0o755)
        env = dict(os.environ, PATH=str(mock_bin) + ":" + os.environ["PATH"],
                   TMPDIR=str(root), NICTUI_TEST_COMMAND=str(root / "command-path"),
                   NICTUI_TEST_ARGS=str(root / "args"))
        for args in ([], ["tui", "--ble-name", "TD-H3; $(echo wrong) ' quoted"]):
            subprocess.run([str(launcher), *args], env=env, check=True,
                           stdin=subprocess.DEVNULL, capture_output=True)
            command = Path((root / "command-path").read_text())
            subprocess.run([str(command)], env=env, check=True, capture_output=True)
            assert json.loads((root / "args").read_text()) == args
            assert not command.exists(), "Launch command was not cleaned up"


if __name__ == "__main__":
    binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/nictui").resolve())
    terminal_checks(binary)
    launcher_checks()
    for size in (32, 33):
        flash_checks(binary, size)
    flash_checks(binary, 33, ack_init=False)
    with tempfile.NamedTemporaryFile(suffix=".bin") as image:
        result = subprocess.run([binary, "firmware", "flash", "--input", image.name,
                                 "--validate-only"], capture_output=True)
        assert result.returncode != 0, "Empty firmware passed validation"
    print("PASS: startup, resize, terminal restore, app launcher, flash ACK retry, padding, empty image")
