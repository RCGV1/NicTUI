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

Then start NicTUI with:
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
~/.local/bin/nictui --uninstall
```

## Usage

1. Connect your TD-H3 radio to the computer via USB
2. Select the appropriate serial port
3. Use the arrow keys to navigate between tabs
4. Press the indicated keys to perform actions

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

MIT License

## Links

- [Source Code](https://github.com/nicradio/NicTUI)
- [Protocol Documentation](https://github.com/nicsure/nicfw2docs)
- [TIDRADIO TD-H3](https://tidradio.com/products/td-h3)
