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

## Supported Platforms

| Platform | Architecture | Download |
|----------|--------------|----------|
| Linux | x86_64 | `nictui-v{version}-linux-x86_64` |
| Linux | ARM (Raspberry Pi) | `nictui-v{version}-linux-aarch64` |
| macOS | Intel (x86_64) | `nictui-v{version}-macos-x86_64` |
| macOS | Apple Silicon (aarch64) | `nictui-v{version}-macos-aarch64` |
| Windows | x86_64 | `nictui-v{version}-windows-x86_64.exe` |

## Installation

### Linux/macOS
```bash
# Download and extract
tar -xzf nictui-v{version}-{platform}
chmod +x nictui-{version}-{platform}
./nictui-{version}-{platform}
```

### Windows
Download the `.exe` file and run it from Command Prompt or PowerShell.

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
git clone https://github.com/nicradio/NicTUI.git
cd NicTUI
cargo build --release
```

### Dependencies (Linux)

- libwayland-dev
- libwayland-client0
- wayland-protocols
- libasound2-dev

## License

MIT License

## Links

- [Source Code](https://github.com/nicradio/NicTUI)
- [Protocol Documentation](https://github.com/nicsure/nicfw2docs)
- [TIDRADIO TD-H3](https://tidradio.com/products/td-h3)
