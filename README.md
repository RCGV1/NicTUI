# NicTUI - Professional TDH3 Radio Programmer

A terminal-based user interface for programming the TIDRADIO TD-H3 HAM radio.

## Features

- **Channel Management** - Read, edit, and write channels to the radio
- **Settings Configuration** - Modify radio settings via the UI
- **Band Plan Editor** - Edit frequency band plans
- **DTMF Presets** - Manage DTMF signaling presets
- **Scan Presets** - Configure scanning behavior
- **Codeplug Operations** - Import/export full codeplugs
- **Firmware Flashing** - Flash firmware updates
- **Remote Control** - Control the radio remotely via USB serial
- **AI Skill Installer** - Install a bundled Codex or Claude Code skill that drives the NicTUI CLI

## Installation

### Quick Install (All Platforms)

One-command installation that automatically detects your platform and installs NicTUI:

```bash
curl -fsSL https://raw.githubusercontent.com/RCGV1/NicTUI/master/install.sh | bash
```

This will:
- Detect your operating system and architecture
- Download the latest release binary
- Install to `~/.local/bin/nictui`
- Add `~/.local/bin` to your PATH

**After installation, restart your terminal or run:**
```bash
source ~/.bashrc  # or ~/.zshrc, ~/.profile, etc.
```

Then start the interactive UI with:
```bash
nictui
```

### Manual Installation

#### Linux/macOS

```bash
# Download the binary
curl -LO https://github.com/RCGV1/NicTUI/releases/latest/download/nictui-{version}-{platform}

# Example for Linux x86_64:
# curl -LO https://github.com/RCGV1/NicTUI/releases/latest/download/nictui-1.0.0-linux-x86_64

# Make executable
chmod +x nictui-{version}-{platform}

# Move to a directory in your PATH
mkdir -p ~/.local/bin
mv nictui-{version}-{platform} ~/.local/bin/nictui

# Add to PATH (add to ~/.bashrc or ~/.zshrc)
export PATH="$HOME/.local/bin:$PATH"
```

#### Windows

Download the `.exe` file from [Releases](https://github.com/RCGV1/NicTUI/releases) and run it from Command Prompt or PowerShell.

### Update NicTUI

To update to the latest version:

```bash
# Re-run the install script
curl -fsSL https://raw.githubusercontent.com/RCGV1/NicTUI/master/install.sh | bash

# Or manually download the new version and replace the binary
```

### Check Version

```bash
nictui --version
```

### Uninstall

```bash
rm -f ~/.local/bin/nictui
```

## Usage

Running `nictui` with no arguments launches the full-screen TUI.

1. Connect your TD-H3 radio to the computer via USB
2. Select the appropriate serial port
3. Use the arrow keys to navigate between tabs
4. Press the indicated keys to perform actions

When you exit the TUI, NicTUI will print a hint about the bundled AI skill if it detects `codex` or `claude` on your system.

## AI Skill

NicTUI ships with a bundled skill for Codex and Claude Code. The skill uses the NicTUI CLI only, requires NicSure mod firmware on the radio, backs up the radio data it changes, prefers targeted updates over bulk writes, and runs `--validate-only` before real writes.

Install into detected agent directories:

```bash
nictui skill install
```

Inspect the bundled skill and install paths:

```bash
nictui skill show
nictui skill paths
```

Force a specific target:

```bash
nictui skill install --agent codex
nictui skill install --agent claude
nictui skill install --agent all
```

Installed locations:

- Codex: `$CODEX_HOME/skills/nictui-radio-cli` or `~/.codex/skills/nictui-radio-cli`
- Claude Code: `~/.claude/skills/nictui-radio-cli`

### Publishing and Discovery

This repo also includes a Claude Code marketplace definition at [.claude-plugin/marketplace.json](.claude-plugin/marketplace.json) and a plugin package at [plugins/nictui-radio-cli-plugin](plugins/nictui-radio-cli-plugin). That gives Claude Code users a directory-style install path through the official plugin marketplace flow.

For local validation and install testing:

```bash
claude plugin validate .
claude plugin marketplace add ./ --scope user
claude plugin install nictui-radio-cli-plugin@nictui-marketplace
```

For Codex, the bundled skill lives at [skills/nictui-radio-cli](skills/nictui-radio-cli). Codex does not currently expose a first-party skill marketplace in this repo, so the practical distribution path is to host the skill in a public GitHub repository and submit it to a community registry that indexes GitHub-hosted Codex skills.

## CLI

NicTUI now includes a non-interactive CLI for scripting, inspection, and batch workflows.
When `--port` is omitted, NicTUI will automatically select the only detected port or the only likely radio USB serial port if auxiliary Bluetooth/debug ports are also present.
The safest non-interactive workflow is: `ports` -> `probe` -> `read` -> `--validate-only` -> actual write.
If `probe` reports `stock/original` firmware, install NicSure mod firmware before using NicTUI live read/write commands.
For focused edits, prefer the single-record commands like `channels get`, `channels update`, `settings get`, and `settings set`.
Use `ports --verbose` or `ports --json` when you need precise port metadata, and `probe --json` when an agent or script needs structured radio facts.

### Discover Ports

```bash
nictui ports
nictui ports --verbose
nictui ports --json
nictui probe --port /dev/cu.usbserial-210
nictui probe --json
```

### Recommended Health Check

```bash
# Quick read-only check
nictui doctor --port /dev/cu.usbserial-210

# Save JSON artifacts for every readable section
nictui doctor --port /dev/cu.usbserial-210 --output-dir ./doctor-artifacts

# Include the full EEPROM dump and print the report as JSON
nictui doctor --port /dev/cu.usbserial-210 --codeplug --json --output-dir ./doctor-artifacts
```

### Work With Channels

```bash
# Read channels from the radio to CSV
nictui channels read --port /dev/cu.usbserial-210 --output channels.csv

# Read channels as JSON to stdout
nictui channels read --port /dev/cu.usbserial-210

# Validate a CSV or JSON channel file without touching the radio
nictui channels write --input channels.json --validate-only

# Write channels from CSV or JSON back to the radio
nictui channels write --port /dev/cu.usbserial-210 --input channels.csv --reboot
```

### Target One Channel

```bash
# Read one channel as JSON
nictui channels get --port /dev/cu.usbserial-210 --channel 25

# Save one channel as CSV or JSON
nictui channels get --port /dev/cu.usbserial-210 --channel 25 --output channel-25.json
nictui channels get --port /dev/cu.usbserial-210 --channel 25 --output channel-25.csv

# Validate a one-channel patch file without touching the radio
nictui channels update --channel 25 --input channel-25.json --validate-only

# Replace one channel slot from a single CSV row or JSON record
nictui channels update --port /dev/cu.usbserial-210 --channel 25 --input channel-25.json

# Clear one channel slot
nictui channels clear --port /dev/cu.usbserial-210 --channel 25

# Clear an inclusive range of channel slots
nictui channels clear-range --port /dev/cu.usbserial-210 --start 26 --end 198
```

### Read Radio Sections

```bash
nictui settings read --port /dev/cu.usbserial-210 --output settings.json

nictui scan-presets read --port /dev/cu.usbserial-210 --output scan-presets.json

nictui band-plan read --port /dev/cu.usbserial-210 --output band-plan.json

nictui dtmf read --port /dev/cu.usbserial-210 --output dtmf.json
```

### Target One Setting

```bash
# Read one setting by menu number
nictui settings get --port /dev/cu.usbserial-210 --setting 17

# Read one setting by name
nictui settings get --port /dev/cu.usbserial-210 --setting "LCD Brightness"

# Validate one setting change without touching the radio
nictui settings set --setting 17 --value 12 --validate-only

# Update one setting by menu number or name
nictui settings set --port /dev/cu.usbserial-210 --setting 17 --value 12
nictui settings set --port /dev/cu.usbserial-210 --setting "Key Tones" --value Voice
```

### Validate Before Writing

```bash
nictui settings write --input settings.json --validate-only
nictui scan-presets write --input scan-presets.json --validate-only
nictui band-plan write --input band-plan.json --validate-only
nictui dtmf write --input dtmf.json --validate-only
nictui codeplug write --input radio.nfw --validate-only
nictui firmware flash --input firmware.bin --validate-only
```

### Write Radio Sections

```bash
nictui settings write --port /dev/cu.usbserial-210 --input settings.json
nictui scan-presets write --port /dev/cu.usbserial-210 --input scan-presets.json
nictui band-plan write --port /dev/cu.usbserial-210 --input band-plan.json
nictui dtmf write --port /dev/cu.usbserial-210 --input dtmf.json
```

### Target One Indexed Record

```bash
# Read one scan preset, band plan, or DTMF preset
nictui scan-presets get --port /dev/cu.usbserial-210 --index 2
nictui band-plan get --port /dev/cu.usbserial-210 --index 4
nictui dtmf get --port /dev/cu.usbserial-210 --index 1

# Validate one record update without touching the radio
nictui scan-presets update --index 2 --input scan-preset-2.json --validate-only
nictui band-plan update --index 4 --input band-plan-4.json --validate-only
nictui dtmf update --index 1 --input dtmf-1.json --validate-only

# Update one record in place
nictui scan-presets update --port /dev/cu.usbserial-210 --index 2 --input scan-preset-2.json
nictui band-plan update --port /dev/cu.usbserial-210 --index 4 --input band-plan-4.json
nictui dtmf update --port /dev/cu.usbserial-210 --index 1 --input dtmf-1.json
```

### Read, Inspect, or Write Codeplugs

```bash
# Read the full EEPROM into a .nfw file
nictui codeplug read --port /dev/cu.usbserial-210 --output radio.nfw

# Inspect a codeplug summary
nictui codeplug inspect --input radio.nfw

# Dump the full inspection payload as JSON
nictui codeplug inspect --input radio.nfw --json

# Validate a .nfw file before writing it
nictui codeplug write --input radio.nfw --validate-only

# Write a .nfw file back to the radio
nictui codeplug write --port /dev/cu.usbserial-210 --input radio.nfw
```

### Flash Firmware

```bash
# Validate the firmware image first
nictui firmware flash --input firmware.bin --validate-only

# Flash the validated image
nictui firmware flash --port /dev/cu.usbserial-210 --input firmware.bin
```

### Serial Port Notes

- Only one NicTUI or serial tool can own `/dev/cu.*` at a time.
- If a command says the port is busy, close other NicTUI sessions, serial monitors, or flashing tools first.
- For repeatable scripting, prefer `/dev/cu.*` instead of `/dev/tty.*` on macOS.

### Remote CLI

```bash
nictui remote key --port /dev/cu.usbserial-210 --key flashlight
nictui remote key --port /dev/cu.usbserial-210 --key ptt-a
```

### Full Help

```bash
nictui --help
nictui channels --help
nictui settings write --help
nictui codeplug inspect --help
```

### Key Bindings

| Key | Action |
|-----|--------|
| `Tab` | Next tab |
| `Shift+Tab` | Previous tab |
| `1-8` | Jump to specific tab |
| `q` | Quit application |
| `r` | Read from radio |
| `w` | Write to radio |
| `Enter` | Edit selected item |
| `Esc` | Cancel / Go back |

### Tab-Specific Bindings

**Channels Tab:**
- `n` - Add new channel
- `e` - Edit channel
- `d` - Delete channel

**Settings Tab:**
- `Enter` - Edit setting

**Scanning Tab:**
- `Enter` - Edit scan preset

**Band Plan Tab:**
- `Enter` - Edit band plan

**DTMF Tab:**
- `Enter` - Edit DTMF preset

**Remote Tab:**
- `o` - Connect to radio
- `p` - Disconnect from radio
- `0-9` - Radio keys
- `a` - PTT A
- `b` - PTT B
- `f` - Flashlight

**Codeplug Tab:**
- `i` - Import codeplug
- `e` - Export codeplug
- `w` - Write codeplug

**BIN Flash Tab:**
- `i` - Import firmware file
- `f` - Start flash

## UI Capture Pipeline

For TUI polish work on macOS, NicTUI includes a local capture script that can launch the debug TUI, send navigation keys, and save a screenshot plus JSON metadata.

Example:

```bash
python3 scripts/tui_capture_macos.py \
  --build \
  --port /dev/cu.usbserial-2110 \
  --keys "r,wait:2,down,down" \
  --output .ui-captures/channels-review.png
```

The script prints two paths:

- the captured PNG
- a JSON file with the launch command, key script, and captured window bounds

Useful options:

```bash
python3 scripts/tui_capture_macos.py --help
python3 scripts/tui_capture_macos.py --keys "tab,wait:1,down"
python3 scripts/tui_capture_macos.py --keep-window
```

Key script tokens support single characters plus named keys such as `up`, `down`, `left`, `right`, `tab`, `backtab`, `enter`, `esc`, and `wait:SECONDS`.

If you use `--keys`, macOS must allow `osascript` / Terminal under **System Settings > Privacy & Security > Accessibility** so the script can send navigation keystrokes into the TUI.

For Codex-driven UI iteration, run the capture script, then attach the generated PNG in the next prompt or reference the saved path directly.

## Building from Source

```bash
git clone https://github.com/RCGV1/NicTUI.git
cd NicTUI
cargo build --release
```

### Dependencies (Linux)

- libgtk-3-dev
- libudev-dev

## License

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.

## Links

- [Source Code](https://github.com/nicradio/NicTUI)
- [Protocol Documentation](https://github.com/nicsure/nicfw2docs)
- [TIDRADIO TD-H3](https://tidradio.com/products/td-h3)
