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


def read_until(fd, marker, seconds=2):
    result = b""
    deadline = time.monotonic() + seconds
    while marker not in result and time.monotonic() < deadline:
        if select.select([fd], [], [], max(0, deadline - time.monotonic()))[0]:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            result += chunk
    assert marker in result, f"Timed out waiting for {marker!r}: {result!r}"
    return result


def wait_for_exit(pid, seconds=2, fd=None):
    assert pid > 0, "Refusing to wait for a process group"
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        exited, status = os.waitpid(pid, os.WNOHANG)
        if exited:
            return status
        if fd is None:
            time.sleep(0.01)
        else:
            # macOS can delay PTY child exit until its final output is drained.
            read_for(fd, min(0.05, max(0, deadline - time.monotonic())))
    raise AssertionError("TUI did not exit before the deadline")


def cleanup_terminal_child(pid, fd):
    original_error = sys.exc_info()[1]
    errors = []
    # Release the PTY before reaping: macOS may hold a child in kernel exit
    # while its master remains open, even after SIGKILL.
    try:
        os.close(fd)
    except OSError as error:
        errors.append(str(error))
    if pid is not None:
        if pid <= 0:
            errors.append("Refusing to signal a non-positive child PID")
        else:
            try:
                exited, _ = os.waitpid(pid, os.WNOHANG)
                if not exited:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    wait_for_exit(pid)
            except ChildProcessError:
                pass
            except (OSError, AssertionError) as error:
                errors.append(str(error))
    if errors:
        message = "Terminal cleanup: " + "; ".join(errors)
        if original_error is None:
            raise AssertionError(message)
        if hasattr(original_error, "add_note"):
            original_error.add_note(message)
        else:
            print(message, file=sys.stderr)


def terminal_checks(binary, view="channels"):
    pid, fd = pty.fork()
    if pid == 0:
        # This baseline PTY does not emulate capability replies or key-up events.
        os.environ["TERM"] = "xterm-256color"
        os.environ["TERM_PROGRAM"] = "Apple_Terminal"
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
        os.execv(binary, [binary, "tui", "--demo", "--demo-view", view])
    try:
        # Terminal diffs may position the shortcut separately from its view label.
        label = b"[A]" if view == "activity" else b"Channels"
        output = read_until(fd, label, seconds=10)
        if view == "activity":
            assert b"Activity" in output, "Activity view did not draw its title"
        assert b"NicTUI" in output, "Startup did not draw the demo UI"
        assert b"\x1b[?u" not in output, "Baseline PTY queried keyboard enhancements"
        assert b"\x1b[?1003h" not in output, "Mouse reporting was enabled"
        if view == "activity":
            os.write(fd, b"s")
            output = read_until(fd, b"Listening")
            assert b"\x1b[?1003h" not in output, "Activity enabled mouse reporting"
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        os.kill(pid, signal.SIGWINCH)
        assert read_for(fd, 1), "Resize did not redraw without a keypress"
        os.write(fd, b"q")
        output = read_until(fd, b"\x1b[?1049l")
        assert b"\x1b[?1049l" in output, "Alternate screen was not restored"
        assert b"\x1b[?1006l" in output, "Mouse reporting was not reset"
        assert termios.tcgetattr(fd)[3] & termios.ICANON, "Raw mode was not restored"
        assert termios.tcgetattr(fd)[3] & termios.ECHO, "Terminal echo was not restored"
        status = wait_for_exit(pid, fd=fd)
        pid = None
        assert status == 0, "TUI exit failed"
    finally:
        cleanup_terminal_child(pid, fd)


def cleanup_flash_child(process, master, slave):
    original_error = sys.exc_info()[1]
    errors = []
    # Release both private PTY descriptors before reaping, including after SIGKILL.
    for fd in (master, slave):
        try:
            os.close(fd)
        except OSError as error:
            errors.append(str(error))
    try:
        if process.poll() is None:
            try:
                process.kill()
            except ProcessLookupError:
                pass
            process.wait(timeout=2)
    except (OSError, subprocess.TimeoutExpired) as error:
        errors.append(str(error))
    finally:
        if process.stdout is not None:
            try:
                process.stdout.close()
            except OSError as error:
                errors.append(str(error))
    if errors:
        message = "Flash cleanup: " + "; ".join(errors)
        if original_error is None:
            raise AssertionError(message)
        if hasattr(original_error, "add_note"):
            original_error.add_note(message)
        else:
            print(message, file=sys.stderr)


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
            block_count = (size + 31) // 32
            for block in range(block_count):
                packet = read_exact(master, 36)
                is_last = block + 1 == block_count
                assert int.from_bytes(packet[1:3], "big") == block
                assert packet[0] == (0xA2 if is_last else 0xA1)
                expected = image[block * 32:(block + 1) * 32].ljust(32, b"\x00")
                assert packet[4:] == expected
                assert packet[3] == sum(expected) % 256
                if block == 0:
                    # A one-block image retries A2; a longer image retries A1.
                    assert read_exact(master, 36) == packet, "Lost ACK skipped a firmware block"
                if is_last:
                    # Silence must not be treated as successful completion.
                    deadline = time.monotonic() + 0.6
                    while time.monotonic() < deadline:
                        assert process.poll() is None, "Flasher exited without the final data ACK"
                        wait = min(0.05, max(0, deadline - time.monotonic()))
                        if select.select([master], [], [], wait)[0]:
                            assert read_exact(master, 36) == packet, "Final ACK retry changed the block"
                    assert process.poll() is None, "Flasher exited without the final data ACK"
                os.write(master, b"\xa3")
            output = process.communicate(timeout=3)[0]
            assert process.returncode == 0, output.decode(errors="replace")
            assert not read_for(master, 0.1), "Flasher sent an extra firmware packet"
        finally:
            cleanup_flash_child(process, master, slave)


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
                          "record = dict(args=sys.argv[1:], env={key: os.environ.get(key) "
                          "for key in ('NO_COLOR', 'TERM', 'COLORTERM')})\n"
                          "open(os.environ['NICTUI_TEST_ARGS'], 'w').write(json.dumps(record))\n")
        binary.chmod(0o755)
        mock_bin = root / "bin"
        mock_bin.mkdir()
        opener = mock_bin / "open"
        opener.write_text('#!/bin/sh\nprintf "%s" "$3" > "$NICTUI_TEST_COMMAND"\n'
                          'exit "${NICTUI_TEST_OPEN_STATUS:-0}"\n')
        opener.chmod(0o755)
        env = dict(os.environ, PATH=str(mock_bin) + ":" + os.environ["PATH"],
                   TMPDIR=str(root), NICTUI_TEST_COMMAND=str(root / "command-path"),
                   NICTUI_TEST_ARGS=str(root / "args"), NO_COLOR="1", COLORTERM="")
        for term in (None, "", "dumb", "xterm-256color", "xterm-ghostty"):
            launch_env = dict(env)
            if term is None:
                launch_env.pop("TERM", None)
            else:
                launch_env["TERM"] = term
            for args in ([], ["tui", "--ble-name", "TD-H3; $(echo wrong) ' quoted"]):
                subprocess.run([str(launcher), *args], env=launch_env, check=True,
                               stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
                command = Path((root / "command-path").read_text())
                subprocess.run([str(command)], env=launch_env, check=True,
                               capture_output=True, timeout=5)
                record = json.loads((root / "args").read_text())
                assert record["args"] == args
                assert record["env"] == dict(NO_COLOR=None, COLORTERM="",
                                             TERM=term if term and term != "dumb" else "xterm-256color")
                assert not command.exists(), "Launch command was not cleaned up"
                assert not command.parent.exists(), "Session directory was not cleaned up"

        # Non-TUI logging and direct terminal invocations keep caller color policy.
        env["TERM"] = "dumb"
        log = root / "logs with spaces" / "app.log"
        (root / "command-path").unlink()
        subprocess.run([str(launcher), "--app-log-file", str(log), "--help"],
                       env=env, check=True, stdin=subprocess.DEVNULL,
                       capture_output=True, timeout=5)
        record = json.loads((root / "args").read_text())
        assert record == dict(args=["--help"], env=dict(NO_COLOR="1", TERM="dumb", COLORTERM=""))
        assert "Args: --help" in log.read_text()
        assert not (root / "command-path").exists(), "CLI unexpectedly opened Terminal"
        master, slave = pty.openpty()
        try:
            subprocess.run([str(launcher), "tui", "--demo"], env=env, check=True,
                           stdin=slave, stdout=slave, stderr=slave, timeout=5)
        finally:
            os.close(master)
            os.close(slave)
        record = json.loads((root / "args").read_text())
        assert record == dict(args=["tui", "--demo"],
                              env=dict(NO_COLOR="1", TERM="dumb", COLORTERM=""))
        assert not (root / "command-path").exists(), "TTY unexpectedly opened Terminal"

        result = subprocess.run([str(launcher)], env=dict(env, NICTUI_TEST_OPEN_STATUS="1"),
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
        assert result.returncode == 1
        command = Path((root / "command-path").read_text())
        assert not command.parent.exists(), "Failed open left a session directory"


if __name__ == "__main__":
    binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/nictui").resolve())
    terminal_checks(binary)
    terminal_checks(binary, view="activity")
    launcher_checks()
    for size in (32, 33):
        flash_checks(binary, size)
    flash_checks(binary, 33, ack_init=False)
    with tempfile.NamedTemporaryFile(suffix=".bin") as image:
        result = subprocess.run([binary, "firmware", "flash", "--input", image.name,
                                 "--validate-only"], capture_output=True)
        assert result.returncode != 0, "Empty firmware passed validation"
    print("PASS: demo startup, Activity start/quit, resize, terminal restore, app launcher, flash ACK retry, final ACK, padding, empty image")
