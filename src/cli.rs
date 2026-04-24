use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ble::{
    BleReadinessReport, BleTarget, assess_ble_readiness, default_scan_timeout,
    disconnect_ble_bridge_for_device, ensure_ble_bridge, parse_ble_device_uri,
    scan_td_h3_ble_devices,
};
use crate::channel_file::{
    ChannelFileFormat, infer_channel_file_format, load_channels_from_path, save_channels_to_writer,
};
use crate::device::{
    FirmwareVariant, PortCandidate, PortKind, ProgressEvent, RemoteMonitorEvent,
    RemoteMonitorOptions, flash_firmware, inspect_codeplug, list_port_candidates, list_ports,
    live_mode_capability, live_mode_hint, monitor_remote, monitor_remote_with_summary, probe_port,
    read_band_plans, read_channels, read_codeplug, read_dtmf_presets, read_group_labels,
    read_scan_presets, read_settings, resolve_port, send_remote_key_with_report,
    update_dtmf_preset, update_scan_preset, validate_band_plans_payload, validate_channels,
    validate_dtmf_presets_payload, validate_scan_presets, write_band_plans, write_channels,
    write_codeplug, write_dtmf_presets, write_group_labels, write_scan_presets, write_settings,
};
use crate::protocol::codeplug::{load_codeplug, save_codeplug};
use crate::protocol::{
    BandPlan, Channel, DTMFPreset, Endianness, RadioProtocol, SETTINGS_METADATA, ScanPreset,
    SettingType, SettingsBlock,
};
use crate::remote::RemoteControlCommand;
use crate::skill::{
    SkillInstallTarget, SupportedAgent, bundled_skill_dir, bundled_skill_markdown, detected_agents,
    install_bundled_skill,
};

const CLI_AFTER_HELP: &str = "\
Safe release workflow:
  1. Find a target:      nictui ports --verbose
  2. Identify firmware:  nictui probe --port <serial>
  3. Capture evidence:   nictui doctor --port <serial> --output-dir ./.live-debug/session --json
  4. Preview writes:     nictui <section> write --input <file> --validate-only
  5. Apply writes:       nictui <section> write --input <file>
  6. Verify afterward:   nictui <section> read --port <serial>

BLE is available through a local bridge, but bridge readiness does not prove
remote/live control. Run `nictui remote diagnose --json`; only
`remote_control_confirmed: true` confirms remote control.";

const BLUETOOTH_AFTER_HELP: &str = "\
BLE workflow:
  nictui bluetooth scan
  nictui bluetooth doctor --name TD-H3
  nictui bluetooth connect --name TD-H3
  nictui probe --ble-name TD-H3

`connect` only validates that NicTUI can open the BLE transport. It does not
prove remote control or live-mode EEPROM access.";

const DOCTOR_AFTER_HELP: &str = "\
Examples:
  nictui doctor --port /dev/cu.usbserial-210 --output-dir ./.live-debug/session
  nictui doctor --ble-name TD-H3 --json

Doctor performs no EEPROM writes. Over USB it may run read-only live-mode timing
checks when available, but it does not run remote-control probes automatically.";

const PROBE_AFTER_HELP: &str = "\
Examples:
  nictui probe --port /dev/cu.usbserial-210
  nictui probe --ble-name TD-H3 --json

Probe identifies the radio and firmware. It does not exercise remote control.";

const REMOTE_AFTER_HELP: &str = "\
Remote evidence notes:
  - RX bytes, telemetry packets, and PVOJH collisions are activity evidence.
  - They are not proof of remote control by themselves.
  - Confirmed control requires a decoded state delta; use:
    nictui remote diagnose --port <serial> --json";

#[derive(Parser, Debug)]
#[command(
    name = "nictui",
    version,
    about = "Program, inspect, and verify a TD-H3 running NicSure firmware",
    after_help = CLI_AFTER_HELP
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Launch the interactive terminal UI
    Tui(TuiArgs),
    /// Print version information
    Version,
    /// List serial ports and identify likely radio devices
    #[command(visible_alias = "list")]
    Ports(PortsArgs),
    /// Probe the radio and report handshake, firmware, endianness, and evidence limits
    #[command(visible_alias = "detect", after_help = PROBE_AFTER_HELP)]
    Probe(ProbeArgs),
    /// Run a radio health check with no EEPROM writes and optionally save artifacts
    #[command(visible_alias = "check", after_help = DOCTOR_AFTER_HELP)]
    Doctor(DoctorArgs),
    /// Inspect or modify memory channels
    Channels {
        #[command(subcommand)]
        command: ChannelsCommand,
    },
    /// Read or write radio settings as JSON
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    /// Scan, diagnose, bridge, or change the radio Bluetooth setting
    #[command(visible_alias = "ble", after_help = BLUETOOTH_AFTER_HELP)]
    Bluetooth {
        #[command(subcommand)]
        command: BluetoothCommand,
    },
    /// Read or write memory group labels
    Groups {
        #[command(subcommand)]
        command: GroupCommand,
    },
    /// Read or write scan presets as JSON
    #[command(alias = "scan")]
    ScanPresets {
        #[command(subcommand)]
        command: ScanPresetCommand,
    },
    /// Read or write band plans as JSON
    #[command(alias = "bandplans")]
    BandPlan {
        #[command(subcommand)]
        command: BandPlanCommand,
    },
    /// Read or write DTMF presets as JSON
    Dtmf {
        #[command(subcommand)]
        command: DtmfCommand,
    },
    /// Read, write, or inspect codeplug files
    Codeplug {
        #[command(subcommand)]
        command: CodeplugCommand,
    },
    /// Flash a raw firmware BIN to the radio
    Firmware {
        #[command(subcommand)]
        command: FirmwareCommand,
    },
    /// Send remote-control probes and classify remote evidence
    #[command(after_help = REMOTE_AFTER_HELP)]
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
    /// Inspect or install the bundled AI radio-control skill
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
}

#[derive(Args, Debug)]
pub struct TuiArgs {
    #[command(flatten)]
    pub port: PortArgs,
}

#[derive(Args, Debug, Clone)]
pub struct PortArgs {
    /// Serial port to use. If omitted, NicTUI auto-selects the only detected radio-like port.
    #[arg(long, conflicts_with_all = ["ble_device", "ble_name"])]
    pub port: Option<String>,
    /// BLE device UUID to bridge through NicTUI's local BLE transport
    #[arg(long, conflicts_with_all = ["port", "ble_name"])]
    pub ble_device: Option<String>,
    /// BLE advertised device name to scan for and bridge locally
    #[arg(long, conflicts_with_all = ["port", "ble_device"])]
    pub ble_name: Option<String>,
    /// Scan timeout in seconds when resolving a BLE device by name
    #[arg(long, default_value_t = 8, requires = "ble_name")]
    pub ble_scan_time: u64,
}

#[derive(Args, Debug)]
pub struct PortsArgs {
    /// Include badges, USB metadata, and probe results in text output
    #[arg(long)]
    pub verbose: bool,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug, Clone)]
pub struct ProbeArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Directory to write diagnostic artifacts and the final doctor report
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
    /// Also read and inspect the full EEPROM codeplug
    #[arg(long)]
    pub codeplug: bool,
    /// Emit the final doctor report as JSON to stdout
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum ChannelsCommand {
    /// Read channels from the radio to CSV or JSON
    #[command(visible_aliases = ["export", "dump"])]
    Read(ReadChannelsArgs),
    /// Write channels from CSV or JSON to the radio
    #[command(visible_aliases = ["import", "apply"])]
    Write(WriteChannelsArgs),
    /// Read one channel by slot number
    #[command(visible_alias = "show")]
    Get(ReadChannelArgs),
    /// Update one channel slot from a single CSV or JSON record
    #[command(visible_aliases = ["set", "edit"])]
    Update(UpdateChannelArgs),
    /// Clear one channel slot
    #[command(visible_aliases = ["erase", "delete"])]
    Clear(ClearChannelArgs),
    /// Clear a contiguous range of channel slots
    #[command(visible_aliases = ["erase-range", "delete-range"])]
    ClearRange(ClearChannelRangeArgs),
}

#[derive(Subcommand, Debug)]
pub enum SettingsCommand {
    /// Read radio settings from the radio into JSON
    #[command(visible_aliases = ["export", "dump"])]
    Read(ReadJsonArgs),
    /// Write radio settings from JSON to the radio
    #[command(visible_aliases = ["import", "apply"])]
    Write(WriteSettingsArgs),
    /// Read one setting by menu number or name
    #[command(visible_alias = "show")]
    Get(ReadSettingArgs),
    /// Update one setting by menu number or name
    #[command(visible_alias = "update")]
    Set(SetSettingArgs),
}

#[derive(Subcommand, Debug)]
pub enum BluetoothCommand {
    /// Scan for BLE radios advertising the TD-H3 service
    Scan(BleScanArgs),
    /// Check BLE readiness before connect/probe and separate desktop permission/runtime blockers from radio-side failures
    Doctor(BleDoctorArgs),
    /// Resolve a BLE radio target and validate the local BLE transport NicTUI uses
    Connect(BleConnectArgs),
    /// Clear any cached BLE target state
    Disconnect(BleDisconnectArgs),
    /// Read the Bluetooth setting and print the current state
    #[command(visible_alias = "get")]
    Status(ReadBluetoothArgs),
    /// Enable Bluetooth on the radio
    #[command(visible_alias = "enable")]
    On(SetBluetoothArgs),
    /// Disable Bluetooth on the radio
    #[command(visible_alias = "disable")]
    Off(SetBluetoothArgs),
}

#[derive(Subcommand, Debug)]
pub enum GroupCommand {
    /// Read all group labels from the radio into JSON
    Read(ReadJsonArgs),
    /// Read one group label by 1-based group number
    Get(ReadGroupArgs),
    /// Update one group label by 1-based group number
    Set(SetGroupArgs),
}

#[derive(Subcommand, Debug)]
pub enum ScanPresetCommand {
    /// Read scan presets from the radio into JSON
    #[command(visible_aliases = ["export", "dump"])]
    Read(ReadJsonArgs),
    /// Write scan presets from JSON to the radio
    #[command(visible_aliases = ["import", "apply"])]
    Write(WriteScanPresetsArgs),
    /// Read one scan preset by index
    #[command(visible_alias = "show")]
    Get(ReadIndexedArgs),
    /// Update one scan preset by index from JSON
    #[command(visible_aliases = ["set", "edit"])]
    Update(UpdateScanPresetArgs),
}

#[derive(Subcommand, Debug)]
pub enum BandPlanCommand {
    /// Read band plans from the radio into JSON
    #[command(visible_aliases = ["export", "dump"])]
    Read(ReadJsonArgs),
    /// Write band plans from JSON to the radio
    #[command(visible_aliases = ["import", "apply"])]
    Write(WriteBandPlansArgs),
    /// Read one band plan by index
    #[command(visible_alias = "show")]
    Get(ReadIndexedArgs),
    /// Update one band plan by index from JSON
    #[command(visible_aliases = ["set", "edit"])]
    Update(UpdateBandPlanArgs),
}

#[derive(Subcommand, Debug)]
pub enum DtmfCommand {
    /// Read DTMF presets from the radio into JSON
    #[command(visible_aliases = ["export", "dump"])]
    Read(ReadJsonArgs),
    /// Write DTMF presets from JSON to the radio
    #[command(visible_aliases = ["import", "apply"])]
    Write(WriteDtmfArgs),
    /// Read one DTMF preset by index
    #[command(visible_alias = "show")]
    Get(ReadIndexedArgs),
    /// Update one DTMF preset by index from JSON
    #[command(visible_aliases = ["set", "edit"])]
    Update(UpdateDtmfArgs),
}

#[derive(Subcommand, Debug)]
pub enum CodeplugCommand {
    /// Read the entire EEPROM from the radio into a .nfw file
    #[command(visible_aliases = ["backup", "dump"])]
    Read(ReadCodeplugArgs),
    /// Write a .nfw file to the radio
    #[command(visible_aliases = ["restore", "apply"])]
    Write(WriteCodeplugArgs),
    /// Print a summary of a .nfw file and optionally include full JSON data
    #[command(visible_alias = "summary")]
    Inspect(InspectCodeplugArgs),
}

#[derive(Subcommand, Debug)]
pub enum FirmwareCommand {
    /// Flash a raw firmware BIN to the radio bootloader
    #[command(visible_aliases = ["write", "apply"])]
    Flash(FlashFirmwareArgs),
}

#[derive(Subcommand, Debug)]
pub enum RemoteCommand {
    /// Send one remote-control key and print the observed evidence
    #[command(visible_alias = "send")]
    Key(RemoteKeyArgs),
    /// Keep a remote session open and print decoded packets; telemetry alone does not confirm control
    #[command(visible_aliases = ["monitor", "watch"])]
    Capture(RemoteMonitorArgs),
    /// Send a preset or raw byte sequence and capture activity without claiming control
    Probe(RemoteProbeArgs),
    /// Run the same probe across several remote-session strategies
    Matrix(RemoteMatrixArgs),
    /// Run built-in probes and confirm control only when a decoded state delta appears
    Diagnose(RemoteDiagnoseArgs),
    /// Sweep read-only PVOJH timing to classify whether live-mode start behaves normally
    PvojhSweep(RemotePvojhSweepArgs),
    /// Experimentally read live-mode 32-byte blocks via the documented PVOJH transaction
    LiveRead(RemoteLiveReadArgs),
    /// Experimentally write one live-mode 32-byte block via the documented PVOJH transaction
    LiveWrite(RemoteLiveWriteArgs),
}

#[derive(Subcommand, Debug)]
pub enum SkillCommand {
    /// Install the bundled NicTUI CLI skill into detected agent directories
    #[command(visible_alias = "sync")]
    Install(InstallSkillArgs),
    /// Print the bundled SKILL.md that NicTUI installs for AI agents
    Show,
    /// Print the expected skill install locations for Codex or Claude Code
    Paths(SkillPathsArgs),
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum SkillAgentChoice {
    Auto,
    Codex,
    Claude,
    All,
}

#[derive(Args, Debug)]
pub struct InstallSkillArgs {
    /// Which AI agent directories to install into
    #[arg(long, value_enum, default_value_t = SkillAgentChoice::Auto)]
    pub agent: SkillAgentChoice,
}

#[derive(Args, Debug)]
pub struct SkillPathsArgs {
    /// Which AI agent directories to report
    #[arg(long, value_enum, default_value_t = SkillAgentChoice::Auto)]
    pub agent: SkillAgentChoice,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ReadChannelsArgs {
    #[command(flatten)]
    pub port: PortArgs,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, value_enum)]
    pub format: Option<ChannelOutputFormat>,
}

#[derive(Args, Debug)]
pub struct WriteChannelsArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Path to the CSV or JSON channel file to write
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Reboot the radio after writing channels
    #[arg(long)]
    pub reboot: bool,
}

#[derive(Args, Debug)]
pub struct ReadChannelArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Channel slot number to read
    #[arg(long)]
    pub channel: u16,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(long, value_enum)]
    pub format: Option<ChannelOutputFormat>,
}

#[derive(Args, Debug)]
pub struct UpdateChannelArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Channel slot number to replace
    #[arg(long)]
    pub channel: u16,
    /// Path to a CSV row or JSON record containing exactly one channel
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Reboot the radio after updating the channel
    #[arg(long)]
    pub reboot: bool,
}

#[derive(Args, Debug)]
pub struct ClearChannelArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Channel slot number to clear
    #[arg(long)]
    pub channel: u16,
    /// Validate the request without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Reboot the radio after clearing the channel
    #[arg(long)]
    pub reboot: bool,
}

#[derive(Args, Debug)]
pub struct ClearChannelRangeArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// First channel slot in the inclusive range to clear
    #[arg(long)]
    pub start: u16,
    /// Last channel slot in the inclusive range to clear
    #[arg(long)]
    pub end: u16,
    /// Validate the request without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Reboot the radio after clearing the range
    #[arg(long)]
    pub reboot: bool,
}

#[derive(Args, Debug)]
pub struct ReadJsonArgs {
    #[command(flatten)]
    pub port: PortArgs,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ReadGroupArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Group number to read (1-16)
    #[arg(long)]
    pub group: u8,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct SetGroupArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Group number to update (1-16)
    #[arg(long)]
    pub group: u8,
    /// Label to store for this group
    #[arg(long)]
    pub label: String,
    /// Validate the request without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Args, Debug)]
pub struct ReadBluetoothArgs {
    #[command(flatten)]
    pub port: PortArgs,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct SetBluetoothArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Parse and validate the requested change without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Skip the reboot that normally follows a settings write
    #[arg(long)]
    pub no_reboot: bool,
}

#[derive(Args, Debug)]
pub struct BleScanArgs {
    /// Scan duration in seconds
    #[arg(long, default_value_t = 8)]
    pub timeout: u64,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct BleDoctorArgs {
    /// BLE device UUID reported by `nictui bluetooth scan`
    #[arg(long, conflicts_with = "name")]
    pub device: Option<String>,
    /// BLE advertised name to scan for, such as TD-H3
    #[arg(long, conflicts_with = "device")]
    pub name: Option<String>,
    /// Scan duration in seconds while checking readiness or resolving a named target
    #[arg(long, default_value_t = 8)]
    pub timeout: u64,
    /// Emit structured JSON instead of text so callers can inspect the failure class and next action
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct BleConnectArgs {
    /// BLE device UUID reported by `nictui bluetooth scan`
    #[arg(long, conflicts_with = "name")]
    pub device: Option<String>,
    /// BLE advertised name to scan for, such as TD-H3
    #[arg(long, conflicts_with = "device")]
    pub name: Option<String>,
    /// Scan duration in seconds when resolving by name
    #[arg(long, default_value_t = 8)]
    pub timeout: u64,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct BleDisconnectArgs {
    /// BLE device UUID reported by `nictui bluetooth scan`
    #[arg(long)]
    pub device: String,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ReadIndexedArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Record index to read
    #[arg(long)]
    pub index: u8,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct WriteSettingsArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Path to the JSON settings file to write
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Skip the reboot that normally follows a settings write
    #[arg(long)]
    pub no_reboot: bool,
}

#[derive(Args, Debug)]
pub struct ReadSettingArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Setting selector: menu number like 17, 03, or a name like "LCD Brightness"
    #[arg(long)]
    pub setting: String,
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct SetSettingArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Setting selector: menu number like 17, 03, or a name like "LCD Brightness"
    #[arg(long)]
    pub setting: String,
    /// New value. Accepts numeric values, boolean forms like on/off, and enum labels.
    #[arg(long)]
    pub value: String,
    /// Parse and validate the requested change without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Skip the reboot that normally follows a settings write
    #[arg(long)]
    pub no_reboot: bool,
}

#[derive(Args, Debug)]
pub struct WriteScanPresetsArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Path to the JSON scan preset file to write
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Args, Debug)]
pub struct UpdateScanPresetArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Scan preset index to replace
    #[arg(long)]
    pub index: u8,
    /// Path to a JSON record containing exactly one scan preset
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Args, Debug)]
pub struct WriteBandPlansArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Path to the JSON band plan file to write
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Args, Debug)]
pub struct UpdateBandPlanArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Band plan index to replace
    #[arg(long)]
    pub index: u8,
    /// Path to a JSON record containing exactly one band plan
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Args, Debug)]
pub struct WriteDtmfArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Path to the JSON DTMF preset file to write
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Args, Debug)]
pub struct UpdateDtmfArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// DTMF preset index to replace
    #[arg(long)]
    pub index: u8,
    /// Path to a JSON record containing exactly one DTMF preset
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the input file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Args, Debug)]
pub struct ReadCodeplugArgs {
    #[command(flatten)]
    pub port: PortArgs,
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(Args, Debug)]
pub struct WriteCodeplugArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Path to the .nfw codeplug file to write
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and inspect the codeplug file without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Skip the reboot that normally follows a codeplug write
    #[arg(long)]
    pub no_reboot: bool,
}

#[derive(Args, Debug)]
pub struct InspectCodeplugArgs {
    #[arg(short, long)]
    pub input: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct FlashFirmwareArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Path to the firmware .bin image to flash
    #[arg(short, long)]
    pub input: PathBuf,
    /// Parse and validate the firmware image without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Args, Debug)]
pub struct RemoteKeyArgs {
    #[command(flatten)]
    pub port: PortArgs,
    #[arg(long, value_enum)]
    pub key: RemoteKey,
}

#[derive(Args, Debug)]
pub struct RemoteMonitorArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// How long to keep the remote session open while collecting packets
    #[arg(long, default_value_t = 8)]
    pub duration: u64,
    /// Include low-level TX/RX serial logs
    #[arg(long)]
    pub raw: bool,
    /// Include idle RX zero-byte logs that are normally suppressed in raw mode
    #[arg(long)]
    pub raw_all: bool,
    /// Send one or more keys during monitoring to exercise the session
    #[arg(long = "send", value_enum)]
    pub send: Vec<RemoteKey>,
    /// Delay between scripted key presses in milliseconds
    #[arg(long, default_value_t = 350)]
    pub send_interval_ms: u64,
    /// Disable the radio before entering remote mode
    #[arg(long)]
    pub disable_radio: bool,
    /// How many times to retry the remote session on failure
    #[arg(long, default_value_t = 0)]
    pub recover_retries: usize,
}

#[derive(Args, Debug)]
pub struct RemoteProbeArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Named logical probe action to send; presets translate to programmer wire bytes automatically
    #[arg(long, value_enum)]
    pub preset: Option<RemoteProbePreset>,
    /// Literal wire bytes to send as-is, for example 0B,00 or 4A 0B 00
    #[arg(long)]
    pub bytes: Option<String>,
    /// Number of times to repeat the sequence
    #[arg(long, default_value_t = 1)]
    pub repeat: u32,
    /// Delay between bytes in milliseconds
    #[arg(long, default_value_t = 80)]
    pub gap_ms: u64,
    /// Additional hold time after each repeated sequence in milliseconds
    #[arg(long, default_value_t = 0)]
    pub hold_ms: u64,
    /// Capture time before sending the probe in milliseconds
    #[arg(long, default_value_t = 250)]
    pub pre_ms: u64,
    /// Capture time after sending the probe in milliseconds
    #[arg(long, default_value_t = 2000)]
    pub post_ms: u64,
    /// Include low-level TX/RX serial logs
    #[arg(long)]
    pub raw: bool,
    /// Include idle RX zero-byte logs that are normally suppressed in raw mode
    #[arg(long)]
    pub raw_all: bool,
    /// Disable the radio before entering remote mode
    #[arg(long)]
    pub disable_radio: bool,
    /// How many times to retry the remote session on failure
    #[arg(long, default_value_t = 0)]
    pub recover_retries: usize,
}

#[derive(Args, Debug)]
pub struct RemoteMatrixArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Named logical probe action to test across multiple remote-session strategies
    #[arg(long, value_enum)]
    pub preset: RemoteProbePreset,
    /// Number of times to repeat the sequence
    #[arg(long, default_value_t = 1)]
    pub repeat: u32,
    /// Delay between bytes in milliseconds
    #[arg(long, default_value_t = 80)]
    pub gap_ms: u64,
    /// Additional hold time after each repeated sequence in milliseconds
    #[arg(long, default_value_t = 0)]
    pub hold_ms: u64,
    /// Capture time before sending the probe in milliseconds
    #[arg(long, default_value_t = 250)]
    pub pre_ms: u64,
    /// Capture time after sending the probe in milliseconds
    #[arg(long, default_value_t = 2000)]
    pub post_ms: u64,
    /// Include low-level TX/RX serial logs
    #[arg(long)]
    pub raw: bool,
    /// Include idle RX zero-byte logs that are normally suppressed in raw mode
    #[arg(long)]
    pub raw_all: bool,
    /// How many times to retry each remote session on failure
    #[arg(long, default_value_t = 0)]
    pub recover_retries: usize,
}

#[derive(Args, Debug)]
pub struct RemoteDiagnoseArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Include low-level TX/RX serial logs
    #[arg(long)]
    pub raw: bool,
    /// Include idle RX zero-byte logs that are normally suppressed in raw mode
    #[arg(long)]
    pub raw_all: bool,
    /// How many times to retry each remote session on failure
    #[arg(long, default_value_t = 1)]
    pub recover_retries: usize,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemotePvojhSweepArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Sweep stage to run
    #[arg(long, value_enum, default_value_t = PvojhSweepStage::StartId)]
    pub stage: PvojhSweepStage,
    /// Comma-separated list of post-magic delays in milliseconds
    #[arg(long, default_value = "0,20,80,250")]
    pub gap_ms: String,
    /// Time to listen after the opener before sending the next stage
    #[arg(long, default_value_t = 50)]
    pub initial_rx_ms: u64,
    /// Time to listen after the final stage
    #[arg(long, default_value_t = 800)]
    pub post_rx_ms: u64,
    /// Cooldown between runs in milliseconds
    #[arg(long, default_value_t = 300)]
    pub cooldown_ms: u64,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteLiveReadArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Start address in the live-mode EEPROM window, for example 0x0CA0
    #[arg(long)]
    pub address: String,
    /// Number of 32-byte blocks to read
    #[arg(long, default_value_t = 1)]
    pub blocks: u16,
    /// Emit structured JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RemoteLiveWriteArgs {
    #[command(flatten)]
    pub port: PortArgs,
    /// Start address in the live-mode EEPROM window, for example 0x0CA0
    #[arg(long)]
    pub address: String,
    /// Exactly 32 bytes of hex data, for example \"00 00 ...\" or \"FF,01,...\"
    #[arg(long)]
    pub bytes: String,
    /// Parse and validate the write request without opening the serial port
    #[arg(long)]
    pub validate_only: bool,
    /// Confirm the experimental live EEPROM write
    #[arg(long, alias = "force")]
    pub yes: bool,
    /// Skip the default readback verification after writing
    #[arg(long)]
    pub no_readback: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum ChannelOutputFormat {
    Csv,
    Json,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum RemoteKey {
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Menu,
    Up,
    Down,
    Exit,
    Star,
    Pound,
    PttA,
    PttB,
    Flashlight,
    Vm,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum RemoteProbePreset {
    Menu,
    Up,
    Down,
    Exit,
    PttA,
    PttB,
    Flashlight,
    Vm,
    HoldMenu,
    HoldPttA,
    TelemetryPrime,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PvojhSweepStage {
    Start,
    StartId,
    CleanupOnly,
}

pub enum Dispatch {
    LaunchTui { port: Option<String> },
    Exit,
}

pub fn dispatch(cli: Cli) -> Result<Dispatch> {
    match cli.command {
        None => Ok(Dispatch::LaunchTui { port: None }),
        Some(Commands::Tui(args)) => Ok(Dispatch::LaunchTui {
            port: resolve_optional_port_for_args(&args.port)?,
        }),
        Some(Commands::Version) => {
            println!("NicTUI {}", env!("CARGO_PKG_VERSION"));
            Ok(Dispatch::Exit)
        }
        Some(Commands::Ports(args)) => {
            run_list_ports(args)?;
            Ok(Dispatch::Exit)
        }
        Some(Commands::Probe(args)) => {
            run_probe(args)?;
            Ok(Dispatch::Exit)
        }
        Some(Commands::Doctor(args)) => {
            run_doctor(args)?;
            Ok(Dispatch::Exit)
        }
        Some(Commands::Channels { command }) => {
            match command {
                ChannelsCommand::Read(args) => run_read_channels(args)?,
                ChannelsCommand::Write(args) => run_write_channels(args)?,
                ChannelsCommand::Get(args) => run_get_channel(args)?,
                ChannelsCommand::Update(args) => run_update_channel(args)?,
                ChannelsCommand::Clear(args) => run_clear_channel(args)?,
                ChannelsCommand::ClearRange(args) => run_clear_channel_range(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::Settings { command }) => {
            match command {
                SettingsCommand::Read(args) => run_read_settings(args)?,
                SettingsCommand::Write(args) => run_write_settings(args)?,
                SettingsCommand::Get(args) => run_get_setting(args)?,
                SettingsCommand::Set(args) => run_set_setting(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::Bluetooth { command }) => {
            match command {
                BluetoothCommand::Scan(args) => run_bluetooth_scan(args)?,
                BluetoothCommand::Doctor(args) => run_bluetooth_doctor(args)?,
                BluetoothCommand::Connect(args) => run_bluetooth_connect(args)?,
                BluetoothCommand::Disconnect(args) => run_bluetooth_disconnect(args)?,
                BluetoothCommand::Status(args) => run_bluetooth_status(args)?,
                BluetoothCommand::On(args) => run_bluetooth_set(args, true)?,
                BluetoothCommand::Off(args) => run_bluetooth_set(args, false)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::Groups { command }) => {
            match command {
                GroupCommand::Read(args) => run_read_groups(args)?,
                GroupCommand::Get(args) => run_get_group(args)?,
                GroupCommand::Set(args) => run_set_group(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::ScanPresets { command }) => {
            match command {
                ScanPresetCommand::Read(args) => run_read_scan_presets(args)?,
                ScanPresetCommand::Write(args) => run_write_scan_presets(args)?,
                ScanPresetCommand::Get(args) => run_get_scan_preset(args)?,
                ScanPresetCommand::Update(args) => run_update_scan_preset(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::BandPlan { command }) => {
            match command {
                BandPlanCommand::Read(args) => run_read_band_plans(args)?,
                BandPlanCommand::Write(args) => run_write_band_plans(args)?,
                BandPlanCommand::Get(args) => run_get_band_plan(args)?,
                BandPlanCommand::Update(args) => run_update_band_plan(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::Dtmf { command }) => {
            match command {
                DtmfCommand::Read(args) => run_read_dtmf(args)?,
                DtmfCommand::Write(args) => run_write_dtmf(args)?,
                DtmfCommand::Get(args) => run_get_dtmf(args)?,
                DtmfCommand::Update(args) => run_update_dtmf(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::Codeplug { command }) => {
            match command {
                CodeplugCommand::Read(args) => run_read_codeplug(args)?,
                CodeplugCommand::Write(args) => run_write_codeplug(args)?,
                CodeplugCommand::Inspect(args) => run_inspect_codeplug(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::Firmware { command }) => {
            match command {
                FirmwareCommand::Flash(args) => run_flash_firmware(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::Remote { command }) => {
            match command {
                RemoteCommand::Key(args) => run_remote_key(args)?,
                RemoteCommand::Capture(args) => run_remote_capture(args)?,
                RemoteCommand::Probe(args) => run_remote_probe(args)?,
                RemoteCommand::Matrix(args) => run_remote_matrix(args)?,
                RemoteCommand::Diagnose(args) => run_remote_diagnose(args)?,
                RemoteCommand::PvojhSweep(args) => run_remote_pvojh_sweep(args)?,
                RemoteCommand::LiveRead(args) => run_remote_live_read(args)?,
                RemoteCommand::LiveWrite(args) => run_remote_live_write(args)?,
            }
            Ok(Dispatch::Exit)
        }
        Some(Commands::Skill { command }) => {
            match command {
                SkillCommand::Install(args) => run_install_skill(args)?,
                SkillCommand::Show => run_show_skill(),
                SkillCommand::Paths(args) => run_skill_paths(args)?,
            }
            Ok(Dispatch::Exit)
        }
    }
}

#[derive(Debug, Serialize)]
struct PortCandidateView {
    port: String,
    kind: String,
    score: i32,
    product: Option<String>,
    manufacturer: Option<String>,
    usb_vid: Option<String>,
    usb_pid: Option<String>,
    handshake: String,
    firmware: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProbeView {
    port: String,
    handshake: String,
    endian: Option<String>,
    channel_endian: Option<String>,
    firmware: Option<String>,
    nicsure_ready: bool,
    live_mode: String,
    remote_capability: String,
    remote_hint: Option<String>,
    hint: Option<String>,
}

#[derive(Debug, Clone)]
struct RemoteCapabilityAssessment {
    status: String,
    detail: String,
}

#[derive(Debug, Serialize)]
struct SkillPathView {
    agent: String,
    detected: bool,
    path: String,
}

fn run_list_ports(args: PortsArgs) -> Result<()> {
    let candidates = list_port_candidates()?;
    if args.json {
        let views = candidates
            .iter()
            .map(port_candidate_view)
            .collect::<Vec<_>>();
        write_json_output(&views, None)?;
        return Ok(());
    }

    if candidates.is_empty() {
        println!("No serial ports found.");
        println!("For BLE radios, run `nictui bluetooth scan`.");
        return Ok(());
    }

    if args.verbose {
        for candidate in candidates {
            println!("{}", format_port_candidate_line(&candidate));
        }
    } else {
        for candidate in candidates {
            println!("{}", candidate.port_name);
        }
    }

    Ok(())
}

fn run_probe(args: ProbeArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let result = probe_port(&port)?;
    let remote = assess_remote_capability(&port, &result, None);

    if args.json {
        write_json_output(&probe_view(&result), None)?;
        return Ok(());
    }

    println!("Port: {}", result.port);
    println!(
        "Handshake: {}",
        if result.handshake_ok { "ok" } else { "failed" }
    );
    if let Some(endian) = result.endian {
        println!("Endian: {:?}", endian);
    }
    if let Some(endian) = result.channel_endian {
        println!("Channel endian: {:?}", endian);
    }
    if let Some(firmware) = result.firmware_variant {
        println!("Firmware: {}", firmware);
    }
    println!("Live mode: {}", live_mode_capability(&result));
    println!("Remote control evidence: {}", remote.status);
    if let Some(hint) = live_mode_hint(&result) {
        println!("Hint: {hint}");
    }
    println!("Remote hint: {}", remote.detail);

    Ok(())
}

fn port_candidate_view(candidate: &PortCandidate) -> PortCandidateView {
    PortCandidateView {
        port: candidate.port_name.clone(),
        kind: candidate.badge().to_string(),
        score: candidate.score,
        product: candidate.product.clone(),
        manufacturer: candidate.manufacturer.clone(),
        usb_vid: candidate.usb_vid.map(|value| format!("{value:04X}")),
        usb_pid: candidate.usb_pid.map(|value| format!("{value:04X}")),
        handshake: if candidate.handshake_ok {
            "ok".to_string()
        } else {
            "not-checked".to_string()
        },
        firmware: candidate.firmware_variant.map(|value| value.to_string()),
    }
}

fn probe_view(result: &crate::device::ProbeResult) -> ProbeView {
    let firmware = result.firmware_variant.map(|value| value.to_string());
    let nicsure_ready = matches!(result.firmware_variant, Some(FirmwareVariant::NicSure));
    let remote = assess_remote_capability(&result.port, result, None);
    ProbeView {
        port: result.port.clone(),
        handshake: if result.handshake_ok {
            "ok".to_string()
        } else {
            "failed".to_string()
        },
        endian: result.endian.map(|value| format!("{value:?}")),
        channel_endian: result.channel_endian.map(|value| format!("{value:?}")),
        firmware,
        nicsure_ready,
        live_mode: live_mode_capability(result).to_string(),
        remote_capability: remote.status,
        remote_hint: Some(remote.detail),
        hint: live_mode_hint(result),
    }
}

fn assess_remote_capability(
    port: &str,
    probe: &crate::device::ProbeResult,
    live_mode_status: Option<&str>,
) -> RemoteCapabilityAssessment {
    if !probe.handshake_ok {
        return RemoteCapabilityAssessment {
            status: "unavailable".to_string(),
            detail: "Handshake failed, so remote capability could not be evaluated.".to_string(),
        };
    }

    if matches!(probe.firmware_variant, Some(FirmwareVariant::Stock)) {
        return RemoteCapabilityAssessment {
            status: "unsupported".to_string(),
            detail:
                "Stock/original firmware does not support NicTUI's NicSure remote/live workflows."
                    .to_string(),
        };
    }

    if !matches!(probe.firmware_variant, Some(FirmwareVariant::NicSure)) {
        return RemoteCapabilityAssessment {
            status: "unknown".to_string(),
            detail:
                "Probe identified a radio, but remote capability remains unknown on this firmware."
                    .to_string(),
        };
    }

    if port.starts_with("ble://") {
        return RemoteCapabilityAssessment {
            status: "not-evaluated".to_string(),
            detail:
                "Probe/doctor do not derive a remote capability conclusion over BLE. Use USB serial plus `remote diagnose --json` if you need explicit control evidence."
                    .to_string(),
        };
    }

    match live_mode_status {
        Some("remote-collision") => RemoteCapabilityAssessment {
            status: "not-confirmed".to_string(),
            detail:
                "The public PVOJH opener collides with remote-mode parsing on this firmware, so the remote parser is reachable. Doctor does not auto-run remote-control probes; treat remote control as unconfirmed until `remote diagnose --json` reports `remote_control_confirmed: true`. `telemetry-primed` and `primed-telemetry-carrythrough` are telemetry-only outcomes."
                    .to_string(),
        },
        Some("supported") => RemoteCapabilityAssessment {
            status: "not-evaluated".to_string(),
            detail:
                "Doctor confirmed the public live-mode handshake, but that does not confirm remote control. Run `remote diagnose --json` if you need an explicit remote-control conclusion."
                    .to_string(),
        },
        Some("timeout") => RemoteCapabilityAssessment {
            status: "unknown".to_string(),
            detail:
                "Doctor saw no live-mode response, so remote capability remains unknown. Run `remote diagnose --json` manually if you need remote evidence."
                    .to_string(),
        },
        Some(other) => RemoteCapabilityAssessment {
            status: "unknown".to_string(),
            detail: format!(
                "Doctor saw live-mode status `{other}`, so remote capability remains unknown. Run `remote diagnose --json` manually if you need remote evidence."
            ),
        },
        None => RemoteCapabilityAssessment {
            status: "not-evaluated".to_string(),
            detail:
                "Probe does not exercise remote control. Use `doctor` for a no-EEPROM-write USB conclusion or `remote diagnose --json` for explicit control evidence."
                    .to_string(),
        },
    }
}

fn format_port_candidate_line(candidate: &PortCandidate) -> String {
    let mut details = vec![format!("[{}]", candidate.badge())];

    if let (Some(vid), Some(pid)) = (candidate.usb_vid, candidate.usb_pid) {
        details.push(format!("VID:PID={vid:04X}:{pid:04X}"));
    }
    if let Some(product) = &candidate.product {
        details.push(format!("product={product}"));
    }
    if let Some(manufacturer) = &candidate.manufacturer {
        details.push(format!("manufacturer={manufacturer}"));
    }
    if candidate.handshake_ok {
        details.push("handshake=ok".to_string());
    }
    if let Some(firmware) = candidate.firmware_variant {
        details.push(format!("firmware={firmware}"));
    }
    if matches!(candidate.kind, PortKind::System | PortKind::Unknown) {
        details.push(format!("score={}", candidate.score));
    }

    format!("{} {}", candidate.port_name, details.join(" "))
}

fn run_read_channels(args: ReadChannelsArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (channels, _) = read_channels(&port, |event| progress.handle(event))?;

    let format = args.format.unwrap_or_else(|| match args.output.as_deref() {
        Some(path) => match infer_channel_file_format(path) {
            Ok(ChannelFileFormat::Csv) => ChannelOutputFormat::Csv,
            _ => ChannelOutputFormat::Json,
        },
        None => ChannelOutputFormat::Json,
    });

    write_channels_output(&channels, args.output.as_deref(), format)?;
    if let Some(output) = args.output {
        eprintln!("Wrote {} channels to {}", channels.len(), output.display());
    }
    Ok(())
}

fn run_write_channels(args: WriteChannelsArgs) -> Result<()> {
    let channels = load_channels_from_path(&args.input)
        .with_context(|| format!("Failed to load {}", args.input.display()))?;
    validate_channels(&channels)?;
    if args.validate_only {
        print_channel_validation_summary(&args.input, &channels);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    write_channels(
        &port,
        &channels,
        &[],
        crate::protocol::Endianness::Big,
        args.reboot,
        |event| progress.handle(event),
    )?;
    eprintln!("Wrote {} channels to {}", channels.len(), port);
    Ok(())
}

fn run_get_channel(args: ReadChannelArgs) -> Result<()> {
    validate_channel_number(args.channel)?;

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (channels, _) = read_channels(&port, |event| progress.handle(event))?;
    let channel = channels
        .into_iter()
        .find(|channel| channel.channel_num == args.channel)
        .with_context(|| format!("Channel {} is empty or not programmed", args.channel))?;

    write_single_channel_output(&channel, args.output.as_deref(), args.format)?;
    Ok(())
}

fn run_update_channel(args: UpdateChannelArgs) -> Result<()> {
    validate_channel_number(args.channel)?;

    let mut channel = load_single_channel_from_path(&args.input)?;
    channel.channel_num = args.channel;
    validate_channels(std::slice::from_ref(&channel))?;

    if args.validate_only {
        print_targeted_channel_validation_summary(&args.input, &channel);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    write_channels(
        &port,
        std::slice::from_ref(&channel),
        &[],
        crate::protocol::Endianness::Big,
        args.reboot,
        |event| progress.handle(event),
    )?;
    eprintln!("Updated channel {} on {}", args.channel, port);
    Ok(())
}

fn run_clear_channel(args: ClearChannelArgs) -> Result<()> {
    validate_channel_number(args.channel)?;

    if args.validate_only {
        println!("Validated clear request for channel {}", args.channel);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    write_channels(
        &port,
        &[],
        &[args.channel],
        crate::protocol::Endianness::Big,
        args.reboot,
        |event| progress.handle(event),
    )?;
    eprintln!("Cleared channel {} on {}", args.channel, port);
    Ok(())
}

fn run_clear_channel_range(args: ClearChannelRangeArgs) -> Result<()> {
    let channels = validate_channel_range(args.start, args.end)?;

    if args.validate_only {
        println!(
            "Validated clear range {}-{} ({} channels)",
            args.start,
            args.end,
            channels.len()
        );
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    write_channels(
        &port,
        &[],
        &channels,
        crate::protocol::Endianness::Big,
        args.reboot,
        |event| progress.handle(event),
    )?;
    eprintln!(
        "Cleared channels {}-{} ({} channels) on {}",
        args.start,
        args.end,
        channels.len(),
        port
    );
    Ok(())
}

fn run_read_settings(args: ReadJsonArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (settings, _) = read_settings(&port, |event| progress.handle(event))?;
    write_json_output(&settings, args.output.as_deref())
}

fn run_write_settings(args: WriteSettingsArgs) -> Result<()> {
    let settings: SettingsBlock = read_json_input(&args.input)?;
    validate_settings_payload(&settings)?;
    if args.validate_only {
        print_settings_validation_summary(&args.input, &settings);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let endian = detect_radio_endianness(&port)?;
    let mut progress = ProgressPrinter::default();
    write_settings(&port, &settings, endian, !args.no_reboot, |event| {
        progress.handle(event)
    })
}

fn run_get_setting(args: ReadSettingArgs) -> Result<()> {
    let setting_index = resolve_setting_selector(&args.setting)?;
    let view = read_setting_view(&args.port, setting_index)?;
    write_json_output(&view, args.output.as_deref())
}

fn run_set_setting(args: SetSettingArgs) -> Result<()> {
    let setting_index = resolve_setting_selector(&args.setting)?;
    let setting_value = parse_setting_value(setting_index, &args.value)?;

    update_setting_value(
        &args.port,
        setting_index,
        setting_value,
        args.validate_only,
        args.no_reboot,
    )
}

fn run_bluetooth_scan(args: BleScanArgs) -> Result<()> {
    let devices = scan_td_h3_ble_devices(Duration::from_secs(args.timeout.max(1)))?;
    if args.json {
        write_json_output(&devices, None)?;
        return Ok(());
    }

    if devices.is_empty() {
        println!("No TD-H3 BLE radios found in {}s.", args.timeout.max(1));
        println!(
            "If Bluetooth is disabled on the radio, enable it over USB with `nictui bluetooth on --port <serial>`."
        );
        if cfg!(target_os = "macos") {
            println!(
                "macOS note: if Bluetooth permission has not been granted yet, launch NicTUI.app once outside hosted wrappers and retry."
            );
        }
        return Ok(());
    }

    for device in devices {
        let rssi = device
            .rssi
            .map(|value| format!(" (rssi={value})"))
            .unwrap_or_default();
        println!(
            "{}{}  {}",
            device.device_id,
            rssi,
            device.name.unwrap_or_else(|| "<unnamed>".to_string())
        );
    }

    if cfg!(target_os = "macos") {
        println!(
            "macOS note: BLE discovery can still fail later until NicTUI.app has received Bluetooth permission."
        );
    }

    Ok(())
}

fn run_bluetooth_doctor(args: BleDoctorArgs) -> Result<()> {
    let target = optional_ble_target_from_options(args.device.as_deref(), args.name.as_deref())?;
    let report = assess_ble_readiness(target.as_ref(), Duration::from_secs(args.timeout.max(1)));

    if args.json {
        write_json_output(&report, None)?;
    } else {
        print_ble_readiness_report(&report);
    }

    if report.ok {
        Ok(())
    } else {
        bail!("{}", report.summary)
    }
}

fn run_bluetooth_connect(args: BleConnectArgs) -> Result<()> {
    let target = ble_target_from_options(args.device.as_deref(), args.name.as_deref())?;
    let bridge = ensure_ble_bridge(&target, Duration::from_secs(args.timeout.max(1)))?;

    if args.json {
        write_json_output(&bridge, None)?;
        return Ok(());
    }

    println!("BLE transport validated: {}", bridge.device_id);
    println!("Use this target as: {}", bridge.tty_path);
    println!(
        "This validates the BLE connection only; it does not start a persistent bridge or confirm remote control/live-mode EEPROM access."
    );

    Ok(())
}

fn run_bluetooth_disconnect(args: BleDisconnectArgs) -> Result<()> {
    let bridge = disconnect_ble_bridge_for_device(args.device.trim())?;
    if args.json {
        write_json_output(&bridge, None)?;
        return Ok(());
    }

    println!("Cleared BLE transport state for {}", bridge.device_id);

    Ok(())
}

fn run_bluetooth_status(args: ReadBluetoothArgs) -> Result<()> {
    let view = read_setting_view(&args.port, bluetooth_setting_index())?;
    write_json_output(&view, args.output.as_deref())
}

fn run_bluetooth_set(args: SetBluetoothArgs, enabled: bool) -> Result<()> {
    update_setting_value(
        &args.port,
        bluetooth_setting_index(),
        if enabled { 1 } else { 0 },
        args.validate_only,
        args.no_reboot,
    )
}

fn read_setting_view(port_args: &PortArgs, setting_index: usize) -> Result<SettingView> {
    let port = resolve_port_for_args(port_args)?;
    let mut progress = ProgressPrinter::default();
    let (settings, _) = read_settings(&port, |event| progress.handle(event))?;
    Ok(build_setting_view(&settings, setting_index))
}

fn update_setting_value(
    port_args: &PortArgs,
    setting_index: usize,
    setting_value: u32,
    validate_only: bool,
    no_reboot: bool,
) -> Result<()> {
    if validate_only {
        print_setting_change_validation_summary(setting_index, setting_value);
        return Ok(());
    }

    let port = resolve_port_for_args(port_args)?;
    let mut read_progress = ProgressPrinter::default();
    let (mut settings, endian) = read_settings(&port, |event| read_progress.handle(event))?;
    settings.set_value(setting_index, setting_value);
    validate_settings_payload(&settings)?;

    let mut write_progress = ProgressPrinter::default();
    write_settings(&port, &settings, endian, !no_reboot, |event| {
        write_progress.handle(event)
    })?;
    eprintln!(
        "Updated setting {} on {}",
        SETTINGS_METADATA[setting_index].menu_num, port
    );
    Ok(())
}

fn run_read_groups(args: ReadJsonArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let labels = read_group_labels(&port, |event| progress.handle(event))?;
    write_json_output(&labels, args.output.as_deref())
}

fn run_get_group(args: ReadGroupArgs) -> Result<()> {
    let group_index = validate_group_number(args.group)?;

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let labels = read_group_labels(&port, |event| progress.handle(event))?;
    let label = labels[group_index].clone();

    #[derive(Serialize)]
    struct GroupLabelView {
        group: u8,
        label: String,
    }

    let view = GroupLabelView {
        group: args.group,
        label,
    };
    write_json_output(&view, args.output.as_deref())
}

fn run_set_group(args: SetGroupArgs) -> Result<()> {
    let group_index = validate_group_number(args.group)?;
    let normalized_label = normalize_group_label(&args.label);

    if args.validate_only {
        println!(
            "Validated group {} label update: {:?}",
            args.group, normalized_label
        );
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut read_progress = ProgressPrinter::default();
    let mut labels = read_group_labels(&port, |event| read_progress.handle(event))?;
    labels[group_index] = normalized_label.clone();

    let mut write_progress = ProgressPrinter::default();
    write_group_labels(&port, &labels, |event| write_progress.handle(event))?;
    eprintln!(
        "Updated group {} label to {:?} on {}",
        args.group, normalized_label, port
    );
    Ok(())
}

fn run_read_scan_presets(args: ReadJsonArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (scan_presets, _) = read_scan_presets(&port, |event| progress.handle(event))?;
    write_json_output(&scan_presets, args.output.as_deref())
}

fn run_write_scan_presets(args: WriteScanPresetsArgs) -> Result<()> {
    let scan_presets: Vec<ScanPreset> = read_json_input(&args.input)?;
    validate_scan_presets(&scan_presets)?;
    if args.validate_only {
        print_scan_preset_validation_summary(&args.input, &scan_presets);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let endian = detect_radio_endianness(&port)?;
    let mut progress = ProgressPrinter::default();
    write_scan_presets(&port, &scan_presets, endian, |event| progress.handle(event))
}

fn run_get_scan_preset(args: ReadIndexedArgs) -> Result<()> {
    ensure_index_in_range(
        args.index,
        crate::protocol::SCAN_PRESET_RECORD_COUNT,
        "scan preset",
    )?;

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (scan_presets, _) = read_scan_presets(&port, |event| progress.handle(event))?;
    let preset = scan_presets
        .into_iter()
        .find(|preset| preset.index == args.index)
        .with_context(|| format!("Scan preset {} was not found", args.index))?;
    write_json_output(&preset, args.output.as_deref())
}

fn run_update_scan_preset(args: UpdateScanPresetArgs) -> Result<()> {
    ensure_index_in_range(
        args.index,
        crate::protocol::SCAN_PRESET_RECORD_COUNT,
        "scan preset",
    )?;

    let mut preset: ScanPreset = read_single_json_or_array_input(&args.input, "scan preset")?;
    preset.index = args.index;
    validate_scan_presets(std::slice::from_ref(&preset))?;

    if args.validate_only {
        print_targeted_index_validation_summary("scan preset", args.index, &args.input);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    update_scan_preset(&port, &preset, |event| progress.handle(event))?;
    eprintln!("Updated scan preset {} on {}", args.index, port);
    Ok(())
}

fn run_read_band_plans(args: ReadJsonArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (band_plans, _) = read_band_plans(&port, |event| progress.handle(event))?;
    write_json_output(&band_plans, args.output.as_deref())
}

fn run_write_band_plans(args: WriteBandPlansArgs) -> Result<()> {
    let band_plans: Vec<BandPlan> = read_json_input(&args.input)?;
    validate_band_plans_payload(&band_plans)?;
    if args.validate_only {
        print_band_plan_validation_summary(&args.input, &band_plans);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let endian = detect_radio_endianness(&port)?;
    let mut progress = ProgressPrinter::default();
    write_band_plans(&port, &band_plans, endian, |event| progress.handle(event))
}

fn run_get_band_plan(args: ReadIndexedArgs) -> Result<()> {
    ensure_index_in_range(
        args.index,
        crate::protocol::BAND_PLAN_RECORD_COUNT,
        "band plan",
    )?;

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (band_plans, _) = read_band_plans(&port, |event| progress.handle(event))?;
    let plan = band_plans
        .into_iter()
        .find(|plan| plan.index == args.index)
        .with_context(|| format!("Band plan {} was not found", args.index))?;
    write_json_output(&plan, args.output.as_deref())
}

fn run_update_band_plan(args: UpdateBandPlanArgs) -> Result<()> {
    ensure_index_in_range(
        args.index,
        crate::protocol::BAND_PLAN_RECORD_COUNT,
        "band plan",
    )?;

    let mut plan: BandPlan = read_single_json_or_array_input(&args.input, "band plan")?;
    plan.index = args.index;
    validate_band_plans_payload(std::slice::from_ref(&plan))?;

    if args.validate_only {
        print_targeted_index_validation_summary("band plan", args.index, &args.input);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    write_band_plans(
        &port,
        std::slice::from_ref(&plan),
        Endianness::Big,
        |event| progress.handle(event),
    )?;
    eprintln!("Updated band plan {} on {}", args.index, port);
    Ok(())
}

fn run_read_dtmf(args: ReadJsonArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let dtmf_presets = read_dtmf_presets(&port, |event| progress.handle(event))?;
    write_json_output(&dtmf_presets, args.output.as_deref())
}

fn run_write_dtmf(args: WriteDtmfArgs) -> Result<()> {
    let dtmf_presets: Vec<DTMFPreset> = read_json_input(&args.input)?;
    validate_dtmf_presets_payload(&dtmf_presets)?;
    if args.validate_only {
        print_dtmf_validation_summary(&args.input, &dtmf_presets);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    write_dtmf_presets(&port, &dtmf_presets, |event| progress.handle(event))
}

fn run_get_dtmf(args: ReadIndexedArgs) -> Result<()> {
    ensure_index_in_range(args.index, 20, "DTMF preset")?;

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let dtmf_presets = read_dtmf_presets(&port, |event| progress.handle(event))?;
    let preset = dtmf_presets
        .into_iter()
        .find(|preset| preset.index == args.index)
        .with_context(|| format!("DTMF preset {} was not found", args.index))?;
    write_json_output(&preset, args.output.as_deref())
}

fn run_update_dtmf(args: UpdateDtmfArgs) -> Result<()> {
    ensure_index_in_range(args.index, 20, "DTMF preset")?;

    let mut preset: DTMFPreset = read_single_json_or_array_input(&args.input, "DTMF preset")?;
    preset.index = args.index;
    validate_dtmf_presets_payload(std::slice::from_ref(&preset))?;

    if args.validate_only {
        print_targeted_index_validation_summary("DTMF preset", args.index, &args.input);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    update_dtmf_preset(&port, &preset, |event| progress.handle(event))?;
    eprintln!("Updated DTMF preset {} on {}", args.index, port);
    Ok(())
}

fn run_read_codeplug(args: ReadCodeplugArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (codeplug, _) = read_codeplug(&port, |event| progress.handle(event))?;
    save_codeplug(&args.output, &codeplug)?;
    eprintln!("Saved codeplug to {}", args.output.display());
    Ok(())
}

fn run_write_codeplug(args: WriteCodeplugArgs) -> Result<()> {
    let codeplug = load_codeplug(&args.input)?;
    let inspection = inspect_codeplug(&codeplug)?;
    if args.validate_only {
        print_codeplug_validation_summary(&args.input, &inspection);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    write_codeplug(&port, &codeplug, !args.no_reboot, |event| {
        progress.handle(event)
    })
}

fn run_inspect_codeplug(args: InspectCodeplugArgs) -> Result<()> {
    let codeplug = load_codeplug(&args.input)?;
    let inspection = inspect_codeplug(&codeplug)?;

    if args.json {
        write_json_output(&inspection, None)
    } else {
        println!("File: {}", args.input.display());
        println!("Size: {} bytes", inspection.size);
        println!("Endian: {:?}", inspection.endian);
        println!("Channel endian: {:?}", inspection.channel_endian);
        println!("VFO memories: {}", inspection.vfo_memory_count);
        println!("Channels: {}", inspection.channel_count);
        println!(
            "Settings: {}",
            if inspection.settings_present {
                "present"
            } else {
                "missing"
            }
        );
        println!("Scan presets: {}", inspection.scan_preset_count);
        println!("Band plans: {}", inspection.band_plan_count);
        println!("DTMF presets: {}", inspection.dtmf_preset_count);
        println!("Unknown regions: {}", inspection.unknown_region_count);
        println!(
            "Unknown regions with live data: {}",
            inspection.unknown_regions_with_live_data
        );
        println!(
            "Named groups: {}",
            inspection
                .group_labels
                .iter()
                .filter(|label| !label.trim().is_empty())
                .count()
        );
        for memory in &inspection.vfo_memories {
            println!(
                "VFO {}: {} / {} {} {}",
                memory.slot,
                memory.channel.rx_freq,
                memory.channel.tx_freq,
                memory.channel.modulation,
                memory.channel.bandwidth
            );
        }
        for region in inspection.regions.iter().filter(|region| {
            matches!(region.kind, crate::device::CodeplugRegionKind::Unknown)
                && region.non_ff_bytes > 0
        }) {
            println!(
                "Unknown data: {} ({}), {} non-FF bytes, first live byte {}",
                format_region_offsets(region.start_offset, region.end_offset_exclusive),
                format_region_size(region.length),
                region.non_ff_bytes,
                region
                    .first_non_ff_offset
                    .map(|offset| format!("0x{offset:04X}"))
                    .unwrap_or_else(|| "n/a".to_string())
            );
            println!("  Preview: {}", region.preview_hex);
        }
        Ok(())
    }
}

fn run_flash_firmware(args: FlashFirmwareArgs) -> Result<()> {
    let firmware = std::fs::read(&args.input)
        .with_context(|| format!("Failed to read {}", args.input.display()))?;
    validate_firmware_image(&args.input, &firmware)?;
    if args.validate_only {
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    flash_firmware(&port, &firmware, |event| progress.handle(event))
}

fn run_remote_key(args: RemoteKeyArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let report = send_remote_key_with_report(&port, args.key.as_key_code()).map_err(|failure| {
        anyhow!(
            "Remote session failed [{}]: {}",
            failure.kind,
            failure.summary
        )
    })?;
    println!("Remote key: {} on {}", report.control.label, port);
    println!("Summary: {}", summarize_remote_key_send_report(&report));
    println!("Evidence: {}", report.control.evidence);
    println!(
        "Command: {} [{}] bytes {}",
        report.control.label, report.control.strategy, report.control.bytes_hex
    );
    if let Some(reaction) = report.control.reaction.as_ref() {
        println!(
            "Reaction: window={}ms rx-first={} surfaced={} unknown={} delta={}",
            reaction.window_ms,
            reaction
                .rx_first_ms
                .map(|millis| format!("{millis}ms"))
                .unwrap_or_else(|| "none".to_string()),
            reaction.surfaced_packets,
            reaction.unknown_packets,
            reaction.deltas
        );
    }
    Ok(())
}

fn summarize_remote_key_send_report(report: &crate::device::RemoteKeySendReport) -> String {
    let Some(reaction) = report.control.reaction.as_ref() else {
        return format!(
            "Sent {}, but no reaction summary was captured.",
            report.control.label
        );
    };

    match report.control.evidence {
        crate::remote::RemoteEvidenceKind::ControlConfirmed => format!(
            "Sent {} and observed {} decoded state delta(s), so remote control is confirmed.",
            report.control.label, reaction.deltas
        ),
        crate::remote::RemoteEvidenceKind::NoControlEvidence => {
            let rx_first = reaction
                .rx_first_ms
                .map(|millis| format!("{millis}ms"))
                .unwrap_or_else(|| "none".to_string());
            format!(
                "Sent {} and observed rx-first={}, surfaced={}, unknown={}, delta={}. Remote control is not yet confirmed.",
                report.control.label,
                rx_first,
                reaction.surfaced_packets,
                reaction.unknown_packets,
                reaction.deltas
            )
        }
        crate::remote::RemoteEvidenceKind::NoTelemetry => format!(
            "Sent {}, but no RX, packets, or decoded state delta were observed in the {}ms reaction window.",
            report.control.label, reaction.window_ms
        ),
        crate::remote::RemoteEvidenceKind::CommandFailed => format!(
            "Sent {}, but the command transport failed before NicTUI could confirm any reaction.",
            report.control.label
        ),
    }
}

fn run_remote_capture(args: RemoteMonitorArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let options = RemoteMonitorOptions {
        duration: std::time::Duration::from_secs(args.duration),
        include_raw_logs: args.raw,
        suppress_idle_zero_logs: !args.raw_all,
        scripted_commands: args
            .send
            .iter()
            .copied()
            .map(RemoteKey::as_command)
            .collect(),
        command_start_delay: std::time::Duration::from_millis(250),
        key_interval: std::time::Duration::from_millis(args.send_interval_ms),
        disable_radio_before_remote: args.disable_radio,
        recover_retries: args.recover_retries,
    };

    eprintln!(
        "Capturing remote packets on {} for {}s. Telemetry alone does not confirm control.",
        port, args.duration
    );

    let packet_count = monitor_remote(&port, &options, |event| {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        match event {
            RemoteMonitorEvent::Status(message) => {
                eprintln!("[{timestamp}] {message}");
            }
            RemoteMonitorEvent::Log(message) => {
                println!("[{timestamp}] {message}");
            }
            RemoteMonitorEvent::Phase(phase) => {
                eprintln!("[{timestamp}] PHASE {phase}");
            }
            RemoteMonitorEvent::Control(report) => {
                let outcome = if report.success { "OK" } else { "ERR" };
                println!(
                    "[{timestamp}] CTRL {outcome} {} [{}] {}",
                    report.label, report.strategy, report.detail
                );
            }
            RemoteMonitorEvent::Delta(delta) => {
                println!("[{timestamp}] DELTA {delta}");
            }
            RemoteMonitorEvent::Packet(packet) => {
                println!("[{timestamp}] REMOTE {}", packet.summary());
            }
        }
    })?;

    eprintln!("Captured {} remote packets from {}", packet_count, port);
    Ok(())
}

fn run_remote_probe(args: RemoteProbeArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    if args.preset.is_some() == args.bytes.is_some() {
        bail!("Pass exactly one of --preset or --bytes for remote probe");
    }
    let command = build_remote_probe_command(
        args.preset,
        args.bytes.as_deref(),
        args.repeat,
        args.gap_ms,
        args.hold_ms,
    )?;
    let duration = std::time::Duration::from_millis(args.pre_ms + args.post_ms + 800)
        + command.estimated_duration();

    let options = RemoteMonitorOptions {
        duration,
        include_raw_logs: args.raw,
        suppress_idle_zero_logs: !args.raw_all,
        scripted_commands: vec![command],
        command_start_delay: std::time::Duration::from_millis(args.pre_ms),
        key_interval: std::time::Duration::from_millis(args.gap_ms),
        disable_radio_before_remote: args.disable_radio,
        recover_retries: args.recover_retries,
    };

    eprintln!(
        "Probing remote session on {} (pre {}ms, post {}ms). Telemetry-only outcomes are common.",
        port, args.pre_ms, args.post_ms
    );

    let packet_count = monitor_remote(&port, &options, |event| {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        match event {
            RemoteMonitorEvent::Status(message) => eprintln!("[{timestamp}] {message}"),
            RemoteMonitorEvent::Log(message) => println!("[{timestamp}] {message}"),
            RemoteMonitorEvent::Phase(phase) => eprintln!("[{timestamp}] PHASE {phase}"),
            RemoteMonitorEvent::Control(report) => {
                let outcome = if report.success { "OK" } else { "ERR" };
                println!(
                    "[{timestamp}] CTRL {outcome} {} [{}] bytes {} | {}",
                    report.label, report.strategy, report.bytes_hex, report.detail
                );
            }
            RemoteMonitorEvent::Delta(delta) => println!("[{timestamp}] DELTA {delta}"),
            RemoteMonitorEvent::Packet(packet) => {
                println!("[{timestamp}] REMOTE {}", packet.summary());
            }
        }
    })?;

    eprintln!("Captured {} remote packets from {}", packet_count, port);
    Ok(())
}

fn run_remote_matrix(args: RemoteMatrixArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let base = args
        .preset
        .as_command(args.repeat, args.gap_ms, args.hold_ms);
    let scenarios = remote_matrix_scenarios(&base, args.gap_ms);

    for (index, scenario) in scenarios.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!(
            "== {} | disable_radio={} | bytes {} ==",
            scenario.label,
            if scenario.disable_radio { "yes" } else { "no" },
            scenario.command.bytes_hex()
        );
        if let Some(note) = scenario.note {
            println!("note: {note}");
        }

        let duration = std::time::Duration::from_millis(args.pre_ms + args.post_ms + 800)
            + scenario.command.estimated_duration();
        let mut delta_count = 0usize;
        let mut packet_count_seen = 0usize;

        let packet_count = monitor_remote(
            &port,
            &RemoteMonitorOptions {
                duration,
                include_raw_logs: args.raw,
                suppress_idle_zero_logs: !args.raw_all,
                scripted_commands: vec![scenario.command.clone()],
                command_start_delay: std::time::Duration::from_millis(args.pre_ms),
                key_interval: std::time::Duration::from_millis(args.gap_ms),
                disable_radio_before_remote: scenario.disable_radio,
                recover_retries: args.recover_retries,
            },
            |event| {
                let timestamp = chrono::Local::now().format("%H:%M:%S");
                match event {
                    RemoteMonitorEvent::Status(message) => {
                        eprintln!("[{timestamp}] {message}");
                    }
                    RemoteMonitorEvent::Log(message) => {
                        println!("[{timestamp}] {message}");
                    }
                    RemoteMonitorEvent::Phase(phase) => {
                        eprintln!("[{timestamp}] PHASE {phase}");
                    }
                    RemoteMonitorEvent::Control(report) => {
                        let outcome = if report.success { "OK" } else { "ERR" };
                        println!(
                            "[{timestamp}] CTRL {outcome} {} [{}] bytes {} | {}",
                            report.label, report.strategy, report.bytes_hex, report.detail
                        );
                    }
                    RemoteMonitorEvent::Delta(delta) => {
                        delta_count += 1;
                        println!("[{timestamp}] DELTA {delta}");
                    }
                    RemoteMonitorEvent::Packet(packet) => {
                        packet_count_seen += 1;
                        println!("[{timestamp}] REMOTE {}", packet.summary());
                    }
                }
            },
        )?;

        println!(
            "SUMMARY {}: surfaced {} packet(s), {} delta(s), monitor count {}",
            scenario.label, packet_count_seen, delta_count, packet_count
        );
        if packet_count_seen == 0 && delta_count == 0 {
            println!(
                "VERDICT {}: no decoded remote telemetry or state delta observed",
                scenario.label
            );
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct RemoteDiagnoseControlView {
    label: String,
    strategy: String,
    bytes_hex: String,
    success: bool,
    detail: String,
    window_ms: u128,
    rx_first_ms: Option<u128>,
    surfaced_packets: usize,
    unknown_packets: usize,
    deltas: usize,
}

#[derive(Debug, Serialize)]
struct RemoteDiagnoseCaseView {
    label: String,
    disable_radio: bool,
    packet_count: usize,
    delta_count: usize,
    telemetry_observed: bool,
    control_delta_observed: bool,
    verdict: String,
    summary: String,
    control: Option<RemoteDiagnoseControlView>,
    failure: Option<RemoteDiagnoseFailureView>,
}

#[derive(Debug, Serialize)]
struct RemoteDiagnoseFailureView {
    kind: String,
    summary: String,
    detail: String,
}

#[derive(Debug, Serialize)]
struct RemoteDiagnoseReport {
    port: String,
    verdict: String,
    summary: String,
    control_delta_observed: bool,
    remote_control_confirmed: bool,
    cases: Vec<RemoteDiagnoseCaseView>,
}

fn run_remote_diagnose(args: RemoteDiagnoseArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let cases = vec![
        run_remote_diagnose_case(
            &port,
            "idle",
            RemoteMonitorOptions {
                duration: Duration::from_millis(1200),
                include_raw_logs: args.raw,
                suppress_idle_zero_logs: !args.raw_all,
                recover_retries: args.recover_retries,
                ..RemoteMonitorOptions::default()
            },
        ),
        run_remote_diagnose_case(
            &port,
            "menu",
            RemoteMonitorOptions {
                duration: Duration::from_millis(1800)
                    + RemoteKey::Menu.as_command().estimated_duration(),
                include_raw_logs: args.raw,
                suppress_idle_zero_logs: !args.raw_all,
                scripted_commands: vec![RemoteKey::Menu.as_command()],
                recover_retries: args.recover_retries,
                ..RemoteMonitorOptions::default()
            },
        ),
        run_remote_diagnose_case(
            &port,
            "hold-menu",
            RemoteMonitorOptions {
                duration: Duration::from_millis(1800)
                    + RemoteControlCommand::held_key(
                        "hold-menu",
                        0x0B,
                        0x00,
                        Duration::from_millis(80),
                        1,
                        Duration::from_millis(1000),
                    )
                    .estimated_duration(),
                include_raw_logs: args.raw,
                suppress_idle_zero_logs: !args.raw_all,
                scripted_commands: vec![RemoteControlCommand::held_key(
                    "hold-menu",
                    0x0B,
                    0x00,
                    Duration::from_millis(80),
                    1,
                    Duration::from_millis(1000),
                )],
                recover_retries: args.recover_retries,
                ..RemoteMonitorOptions::default()
            },
        ),
        run_remote_diagnose_case(
            &port,
            "telemetry-prime",
            RemoteMonitorOptions {
                duration: Duration::from_millis(1800)
                    + telemetry_prime_command().estimated_duration(),
                include_raw_logs: args.raw,
                suppress_idle_zero_logs: !args.raw_all,
                scripted_commands: vec![telemetry_prime_command()],
                recover_retries: args.recover_retries,
                ..RemoteMonitorOptions::default()
            },
        ),
        run_remote_diagnose_case(
            &port,
            "telemetry-prime+hold-menu",
            RemoteMonitorOptions {
                duration: Duration::from_millis(2500)
                    + telemetry_prime_command().estimated_duration()
                    + RemoteControlCommand::held_key(
                        "hold-menu",
                        0x0B,
                        0x00,
                        Duration::from_millis(80),
                        1,
                        Duration::from_millis(1000),
                    )
                    .estimated_duration(),
                include_raw_logs: args.raw,
                suppress_idle_zero_logs: !args.raw_all,
                scripted_commands: vec![
                    telemetry_prime_command(),
                    RemoteControlCommand::held_key(
                        "hold-menu",
                        0x0B,
                        0x00,
                        Duration::from_millis(80),
                        1,
                        Duration::from_millis(1000),
                    ),
                ],
                key_interval: Duration::from_millis(250),
                recover_retries: args.recover_retries,
                ..RemoteMonitorOptions::default()
            },
        ),
        run_remote_diagnose_case(
            &port,
            "hold-menu-disable-radio",
            RemoteMonitorOptions {
                duration: Duration::from_millis(1800)
                    + RemoteControlCommand::held_key(
                        "hold-menu",
                        0x0B,
                        0x00,
                        Duration::from_millis(80),
                        1,
                        Duration::from_millis(1000),
                    )
                    .estimated_duration(),
                include_raw_logs: args.raw,
                suppress_idle_zero_logs: !args.raw_all,
                scripted_commands: vec![RemoteControlCommand::held_key(
                    "hold-menu",
                    0x0B,
                    0x00,
                    Duration::from_millis(80),
                    1,
                    Duration::from_millis(1000),
                )],
                disable_radio_before_remote: true,
                recover_retries: args.recover_retries,
                ..RemoteMonitorOptions::default()
            },
        ),
    ];

    let remote_control_confirmed = cases.iter().any(|case| case.control_delta_observed);
    let report = RemoteDiagnoseReport {
        port,
        verdict: classify_remote_diagnose(&cases).to_string(),
        summary: summarize_remote_diagnose(&cases),
        control_delta_observed: remote_control_confirmed,
        remote_control_confirmed,
        cases,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("Remote diagnose for {}", report.port);
    println!("Verdict: {}", report.verdict);
    println!(
        "Confirmed remote control: {}",
        if report.remote_control_confirmed {
            "yes"
        } else {
            "no"
        }
    );
    println!("Summary: {}", report.summary);
    for case in &report.cases {
        println!();
        println!(
            "{}: verdict={} packets={} deltas={} telemetry={} control-delta={} disable_radio={}",
            case.label,
            case.verdict,
            case.packet_count,
            case.delta_count,
            if case.telemetry_observed { "yes" } else { "no" },
            if case.control_delta_observed {
                "yes"
            } else {
                "no"
            },
            case.disable_radio
        );
        println!("summary: {}", case.summary);
        if let Some(failure) = &case.failure {
            println!(
                "failure: {}: {} ({})",
                failure.kind, failure.summary, failure.detail
            );
        }
        if let Some(control) = &case.control {
            println!(
                "control: {} [{}] bytes {} success={}",
                control.label, control.strategy, control.bytes_hex, control.success
            );
            println!(
                "reaction: window={}ms rx-first={} surfaced={} unknown={} delta={}",
                control.window_ms,
                control
                    .rx_first_ms
                    .map(|millis| format!("{millis}ms"))
                    .unwrap_or_else(|| "none".to_string()),
                control.surfaced_packets,
                control.unknown_packets,
                control.deltas
            );
            println!("detail: {}", control.detail);
        }
    }

    Ok(())
}

fn run_remote_diagnose_case(
    port: &str,
    label: &str,
    options: RemoteMonitorOptions,
) -> RemoteDiagnoseCaseView {
    let mut packet_count = 0usize;
    let mut delta_count = 0usize;
    let mut control = None;
    let result = monitor_remote_with_summary(port, &options, |event| match event {
        RemoteMonitorEvent::Control(report) => control = Some(report),
        RemoteMonitorEvent::Packet(_) => packet_count += 1,
        RemoteMonitorEvent::Delta(_) => delta_count += 1,
        _ => {}
    });
    let (packet_count, failure) = match result {
        Ok(summary) => (summary.packet_count, None),
        Err(error) => (
            packet_count,
            Some(RemoteDiagnoseFailureView {
                kind: error.kind.to_string(),
                summary: error.summary,
                detail: error.detail,
            }),
        ),
    };
    let control = control.map(|report| RemoteDiagnoseControlView {
        label: report.label,
        strategy: report.strategy.to_string(),
        bytes_hex: report.bytes_hex,
        success: report.success,
        detail: report.detail,
        window_ms: report
            .reaction
            .as_ref()
            .map(|reaction| reaction.window_ms)
            .unwrap_or(0),
        rx_first_ms: report
            .reaction
            .as_ref()
            .and_then(|reaction| reaction.rx_first_ms),
        surfaced_packets: report
            .reaction
            .as_ref()
            .map(|reaction| reaction.surfaced_packets)
            .unwrap_or(0),
        unknown_packets: report
            .reaction
            .as_ref()
            .map(|reaction| reaction.unknown_packets)
            .unwrap_or(0),
        deltas: report
            .reaction
            .as_ref()
            .map(|reaction| reaction.deltas)
            .unwrap_or(0),
    });
    let diagnosis = diagnose_remote_case(
        label,
        packet_count,
        delta_count,
        control.as_ref(),
        failure.as_ref(),
    );
    RemoteDiagnoseCaseView {
        label: label.to_string(),
        disable_radio: options.disable_radio_before_remote,
        packet_count,
        delta_count,
        telemetry_observed: diagnosis.telemetry_observed,
        control_delta_observed: diagnosis.control_delta_observed,
        verdict: diagnosis.verdict.to_string(),
        summary: diagnosis.summary,
        control,
        failure,
    }
}

struct RemoteDiagnoseOutcome {
    verdict: &'static str,
    summary: String,
    telemetry_observed: bool,
    control_delta_observed: bool,
}

fn diagnose_remote_case(
    label: &str,
    packet_count: usize,
    delta_count: usize,
    control: Option<&RemoteDiagnoseControlView>,
    failure: Option<&RemoteDiagnoseFailureView>,
) -> RemoteDiagnoseOutcome {
    let telemetry_observed = packet_count > 0
        || delta_count > 0
        || control.is_some_and(|control| {
            control.surfaced_packets > 0
                || control.unknown_packets > 0
                || control.rx_first_ms.is_some()
        });
    let control_delta_observed =
        control.is_some_and(|control| control.success && control.deltas > 0);

    if let Some(failure) = failure {
        return RemoteDiagnoseOutcome {
            verdict: "session-failed",
            summary: format!(
                "The diagnose case failed before completing: {}: {}",
                failure.kind, failure.summary
            ),
            telemetry_observed,
            control_delta_observed: false,
        };
    }

    if label == "telemetry-prime" {
        return if telemetry_observed {
            RemoteDiagnoseOutcome {
                verdict: "telemetry-primed",
                summary:
                    "Prime burst woke telemetry. This proves the session can emit telemetry, not that remote control changed state."
                        .to_string(),
                telemetry_observed,
                control_delta_observed: false,
            }
        } else {
            RemoteDiagnoseOutcome {
                verdict: "no-prime-response",
                summary: "Prime burst produced no observable telemetry.".to_string(),
                telemetry_observed: false,
                control_delta_observed: false,
            }
        };
    }

    let Some(control) = control else {
        return if telemetry_observed {
            RemoteDiagnoseOutcome {
                verdict: "telemetry-present",
                summary: "Remote telemetry was present without sending a probe command."
                    .to_string(),
                telemetry_observed: true,
                control_delta_observed: false,
            }
        } else {
            RemoteDiagnoseOutcome {
                verdict: "silent",
                summary: "No telemetry or command response was observed.".to_string(),
                telemetry_observed: false,
                control_delta_observed: false,
            }
        };
    };

    if !control.success {
        return RemoteDiagnoseOutcome {
            verdict: "command-failed",
            summary: "The probe command itself failed before a reaction could be evaluated."
                .to_string(),
            telemetry_observed,
            control_delta_observed: false,
        };
    }

    if control_delta_observed {
        return RemoteDiagnoseOutcome {
            verdict: "confirmed-control-delta",
            summary: "This command produced at least one decoded state delta, so control activity is confirmed."
                .to_string(),
            telemetry_observed,
            control_delta_observed: true,
        };
    }

    if label == "telemetry-prime+hold-menu" && telemetry_observed {
        let carrythrough = if control.surfaced_packets > 0 || control.unknown_packets > 0 {
            format!(
                "The follow-up control coincided with {} surfaced packet(s) and {} unknown packet(s), but no decoded control delta appeared.",
                control.surfaced_packets, control.unknown_packets
            )
        } else if let Some(first_rx) = control.rx_first_ms {
            format!(
                "The follow-up control only produced raw RX after {first_rx}ms, with no decoded control delta."
            )
        } else {
            "The follow-up control produced no decoded control delta.".to_string()
        };
        return RemoteDiagnoseOutcome {
            verdict: "primed-telemetry-carrythrough",
            summary: format!(
                "Telemetry stayed awake after priming. {carrythrough} Treat this as carrythrough telemetry, not confirmed remote control."
            ),
            telemetry_observed: true,
            control_delta_observed: false,
        };
    }

    if control.surfaced_packets > 0 || control.unknown_packets > 0 {
        RemoteDiagnoseOutcome {
            verdict: "telemetry-after-command-no-control-delta",
            summary:
                "This command surfaced telemetry or unknown packets, but none produced a decoded control delta. Treat it as session activity, not confirmed control."
                    .to_string(),
            telemetry_observed,
            control_delta_observed: false,
        }
    } else if control.rx_first_ms.is_some() {
        RemoteDiagnoseOutcome {
            verdict: "raw-rx-only",
            summary:
                "The radio replied at the byte level, but no packets or state deltas were decoded. Treat it as RX activity, not confirmed control."
                    .to_string(),
            telemetry_observed,
            control_delta_observed: false,
        }
    } else {
        RemoteDiagnoseOutcome {
            verdict: "no-reaction",
            summary: "No observable reaction followed this command.".to_string(),
            telemetry_observed,
            control_delta_observed: false,
        }
    }
}

fn classify_remote_diagnose(cases: &[RemoteDiagnoseCaseView]) -> &'static str {
    let session_failed = cases.iter().any(|case| case.failure.is_some());
    let idle_telemetry = cases.iter().any(|case| case.verdict == "telemetry-present");
    let primed_telemetry = cases
        .iter()
        .any(|case| case.label == "telemetry-prime" && case.telemetry_observed);
    let control_delta_observed = cases
        .iter()
        .filter(|case| case.control.is_some() && case.label != "telemetry-prime")
        .any(|case| case.control_delta_observed);
    let primed_carrythrough = cases
        .iter()
        .any(|case| case.verdict == "primed-telemetry-carrythrough");
    let command_rx_without_delta = cases
        .iter()
        .filter(|case| case.control.is_some() && case.label != "telemetry-prime")
        .any(|case| {
            matches!(
                case.verdict.as_str(),
                "primed-telemetry-carrythrough"
                    | "telemetry-after-command-no-control-delta"
                    | "raw-rx-only"
            )
        });

    if control_delta_observed {
        "confirmed-control-delta"
    } else if session_failed {
        "session-failed"
    } else if primed_carrythrough {
        "primed-telemetry-carrythrough"
    } else if primed_telemetry {
        "primed-telemetry-no-confirmed-control"
    } else if command_rx_without_delta {
        "rx-without-confirmed-control"
    } else if idle_telemetry {
        "telemetry-only"
    } else {
        "no-observable-traffic"
    }
}

fn summarize_remote_diagnose(cases: &[RemoteDiagnoseCaseView]) -> String {
    let verdict = classify_remote_diagnose(cases);
    match verdict {
        "confirmed-control-delta" => {
            "At least one tested command produced a decoded control delta, so remote control is confirmed."
                .to_string()
        }
        "primed-telemetry-carrythrough" => {
            "Telemetry woke after the prime burst and stayed visible during a follow-up command, but the follow-up only showed carrythrough telemetry and no decoded control delta. Treat this as telemetry-only, not confirmed remote control."
                .to_string()
        }
        "primed-telemetry-no-confirmed-control" => {
            "Telemetry woke after the prime burst, but no follow-up command produced a decoded control delta. Treat the observed packets as primed or carrythrough telemetry, not confirmed control."
                .to_string()
        }
        "rx-without-confirmed-control" => {
            "Commands produced RX activity, telemetry, or unknown packets, but no decoded control delta. Remote control is not yet confirmed."
                .to_string()
        }
        "telemetry-only" => {
            "The session carries telemetry, but the tested commands did not show a decoded control delta. Treat this as telemetry-only, not confirmed remote control."
                .to_string()
        }
        "session-failed" => {
            "One or more diagnose cases failed before completing. Inspect the per-case failure fields; remote control is not confirmed."
                .to_string()
        }
        _ => "The session opened, but no observable telemetry or control response was captured."
            .to_string(),
    }
}

fn run_remote_pvojh_sweep(args: RemotePvojhSweepArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    if port.starts_with("ble://") {
        bail!("remote pvojh-sweep currently supports USB serial only");
    }
    ensure_live_mode_supported(&port)?;

    let gaps_ms = parse_u64_list(&args.gap_ms)?;
    if gaps_ms.len() > 24 {
        bail!("Refusing to run more than 24 PVOJH sweep scenarios at once");
    }

    let mut results = Vec::with_capacity(gaps_ms.len());
    for (index, gap_ms) in gaps_ms.iter().copied().enumerate() {
        if index > 0 {
            std::thread::sleep(std::time::Duration::from_millis(args.cooldown_ms));
        }
        results.push(run_pvojh_sweep_case(
            &port,
            args.stage,
            gap_ms,
            args.initial_rx_ms,
            args.post_rx_ms,
        )?);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        for result in results {
            println!(
                "gap={}ms start={} next={} cleanup={} verdict={}",
                result.gap_ms,
                summarize_bytes(&result.start_rx),
                summarize_bytes(&result.next_rx),
                summarize_bytes(&result.cleanup_rx),
                result.verdict
            );
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct PvojhSweepResult {
    stage: PvojhSweepStage,
    gap_ms: u64,
    start_rx: Vec<u8>,
    next_rx: Vec<u8>,
    cleanup_rx: Vec<u8>,
    verdict: String,
}

fn run_pvojh_sweep_case(
    port: &str,
    stage: PvojhSweepStage,
    gap_ms: u64,
    initial_rx_ms: u64,
    post_rx_ms: u64,
) -> Result<PvojhSweepResult> {
    let mut proto = RadioProtocol::new(port)?;
    proto.send_bytes(&[0x50, 0x56, 0x4F, 0x4A, 0x48, 0x5C, 0x14])?;
    let start_rx = collect_probe_bytes(&mut proto, initial_rx_ms)?;
    std::thread::sleep(std::time::Duration::from_millis(gap_ms));

    let next_rx = match stage {
        PvojhSweepStage::Start => Vec::new(),
        PvojhSweepStage::StartId => {
            proto.send_bytes(&[0x02])?;
            collect_probe_bytes(&mut proto, post_rx_ms)?
        }
        PvojhSweepStage::CleanupOnly => {
            proto.send_bytes(&[0x45])?;
            collect_probe_bytes(&mut proto, post_rx_ms)?
        }
    };

    let cleanup_rx = if matches!(stage, PvojhSweepStage::CleanupOnly) {
        Vec::new()
    } else {
        proto.send_bytes(&[0x45])?;
        collect_probe_bytes(&mut proto, 200)?
    };

    Ok(PvojhSweepResult {
        stage,
        gap_ms,
        verdict: classify_pvojh_result(stage, &start_rx, &next_rx),
        start_rx,
        next_rx,
        cleanup_rx,
    })
}

fn collect_probe_bytes(proto: &mut RadioProtocol, listen_ms: u64) -> Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(listen_ms);
    let mut bytes = Vec::new();
    while std::time::Instant::now() < deadline {
        if let Some(byte) = proto.read_byte()? {
            bytes.push(byte);
        }
    }
    Ok(bytes)
}

fn classify_pvojh_result(stage: PvojhSweepStage, start_rx: &[u8], next_rx: &[u8]) -> String {
    if start_rx == [0x06] {
        return "live-ack".to_string();
    }
    if start_rx == [0x4A] && stage == PvojhSweepStage::StartId {
        if next_rx.len() == 8 {
            return "unexpected-id-after-4a".to_string();
        }
        if next_rx == [0x02] {
            return "remote-collision-echo".to_string();
        }
        if next_rx.is_empty() {
            return "remote-collision-swallow".to_string();
        }
    }
    if start_rx.is_empty() && next_rx.is_empty() {
        return "timeout".to_string();
    }
    "partial".to_string()
}

fn summarize_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        "<none>".to_string()
    } else {
        format_hex_line(bytes)
    }
}

fn summarize_pvojh_results(results: &[PvojhSweepResult]) -> (String, String) {
    if results
        .iter()
        .any(|result| result.verdict.as_str() == "live-ack")
    {
        return (
            "supported".to_string(),
            "Observed a documented live-mode ACK during the PVOJH sweep.".to_string(),
        );
    }

    if results.iter().any(|result| {
        matches!(
            result.verdict.as_str(),
            "remote-collision-swallow" | "remote-collision-echo"
        )
    }) {
        let summary = results
            .iter()
            .map(|result| format!("gap={}ms={}", result.gap_ms, result.verdict))
            .collect::<Vec<_>>()
            .join(", ");
        return (
            "remote-collision".to_string(),
            format!(
                "PVOJH opener collides with remote-mode parsing on this firmware ({summary}). Live-mode block access is unavailable through the public handshake."
            ),
        );
    }

    if results
        .iter()
        .all(|result| result.verdict.as_str() == "timeout")
    {
        return (
            "timeout".to_string(),
            "PVOJH sweep produced no start or follow-up bytes.".to_string(),
        );
    }

    let summary = results
        .iter()
        .map(|result| format!("gap={}ms={}", result.gap_ms, result.verdict))
        .collect::<Vec<_>>()
        .join(", ");
    (
        "partial".to_string(),
        format!("PVOJH sweep was inconclusive ({summary})."),
    )
}

fn build_remote_probe_command(
    preset: Option<RemoteProbePreset>,
    bytes: Option<&str>,
    repeat: u32,
    gap_ms: u64,
    hold_ms: u64,
) -> Result<RemoteControlCommand> {
    if let Some(preset) = preset {
        Ok(preset.as_command(repeat, gap_ms, hold_ms))
    } else {
        Ok(RemoteControlCommand::sequence(
            "raw-sequence",
            parse_remote_probe_bytes(bytes)?,
            std::time::Duration::from_millis(gap_ms),
            repeat,
            std::time::Duration::from_millis(hold_ms),
        ))
    }
}

#[derive(Clone)]
struct RemoteMatrixScenario {
    label: String,
    note: Option<&'static str>,
    command: RemoteControlCommand,
    disable_radio: bool,
}

fn remote_matrix_scenarios(base: &RemoteControlCommand, gap_ms: u64) -> Vec<RemoteMatrixScenario> {
    let gap = std::time::Duration::from_millis(gap_ms);
    vec![
        RemoteMatrixScenario {
            label: "baseline".to_string(),
            note: None,
            command: base.clone(),
            disable_radio: false,
        },
        RemoteMatrixScenario {
            label: "disable-radio".to_string(),
            note: None,
            command: base.clone(),
            disable_radio: true,
        },
        RemoteMatrixScenario {
            label: "remote-on-prefix".to_string(),
            note: Some("injects extra 4A REMOTE_ON before the stimulus"),
            command: with_sync_prefix(base.clone(), gap),
            disable_radio: false,
        },
        RemoteMatrixScenario {
            label: "disable+remote-on-prefix".to_string(),
            note: Some("injects extra 4A REMOTE_ON before the stimulus"),
            command: with_sync_prefix(base.clone(), gap),
            disable_radio: true,
        },
        RemoteMatrixScenario {
            label: "remote-on-wrap".to_string(),
            note: Some("injects extra 4A REMOTE_ON before and after the stimulus"),
            command: with_sync_wrap(base.clone(), gap),
            disable_radio: false,
        },
        RemoteMatrixScenario {
            label: "disable+remote-on-wrap".to_string(),
            note: Some("injects extra 4A REMOTE_ON before and after the stimulus"),
            command: with_sync_wrap(base.clone(), gap),
            disable_radio: true,
        },
    ]
}

fn with_sync_prefix(
    mut command: RemoteControlCommand,
    gap: std::time::Duration,
) -> RemoteControlCommand {
    let mut steps = vec![crate::remote::RemoteWriteStep {
        bytes: vec![0x4A],
        pause_after: gap,
    }];
    steps.extend(command.steps);
    command.steps = steps;
    command.label = format!("{}+remote-on", command.label);
    command
}

fn with_sync_wrap(
    mut command: RemoteControlCommand,
    gap: std::time::Duration,
) -> RemoteControlCommand {
    command = with_sync_prefix(command, gap);
    command.steps.push(crate::remote::RemoteWriteStep {
        bytes: vec![0x4A],
        pause_after: gap,
    });
    command.label = format!("{}+post-remote-on", command.label);
    command
}

fn parse_remote_probe_bytes(value: Option<&str>) -> Result<Vec<u8>> {
    let Some(value) = value else {
        bail!("Provide either --preset or --bytes for remote probe");
    };

    let mut bytes = Vec::new();
    for token in value
        .split([',', ' ', '\t', '\n'])
        .filter(|token| !token.trim().is_empty())
    {
        let token = token
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X");
        let byte = u8::from_str_radix(token, 16)
            .map_err(|error| anyhow!("Invalid hex byte '{token}': {error}"))?;
        bytes.push(byte);
    }

    if bytes.is_empty() {
        bail!("Provide at least one byte with --bytes, for example --bytes 0B,00");
    }

    Ok(bytes)
}

fn parse_u64_list(value: &str) -> Result<Vec<u64>> {
    let mut values = Vec::new();
    for token in value
        .split([',', ' ', '\t', '\n'])
        .filter(|token| !token.trim().is_empty())
    {
        values.push(
            token
                .trim()
                .parse::<u64>()
                .map_err(|error| anyhow!("Invalid integer '{token}': {error}"))?,
        );
    }

    if values.is_empty() {
        bail!("Provide at least one integer value");
    }

    Ok(values)
}

fn run_remote_live_read(args: RemoteLiveReadArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let start_address = parse_u16_address(&args.address)?;
    validate_live_block_span(start_address, args.blocks)?;
    let (mut session, id) = LiveModeSession::begin(&port)?;
    let mut blocks = Vec::with_capacity(args.blocks as usize);

    for index in 0..args.blocks {
        let address = start_address.wrapping_add(index * 32);
        let data = session.read_block(address)?;
        blocks.push((address, data));
    }

    let end_id = session.close()?;
    if args.json {
        #[derive(Serialize)]
        struct LiveBlockDump {
            port: String,
            session_id: String,
            end_session_id: String,
            blocks: Vec<LiveBlockJson>,
        }
        #[derive(Serialize)]
        struct LiveBlockJson {
            address: String,
            hex: String,
            decoded: Vec<String>,
        }

        let payload = LiveBlockDump {
            port,
            session_id: format_hex_line(&id),
            end_session_id: format_hex_line(&end_id),
            blocks: blocks
                .into_iter()
                .map(|(address, data)| LiveBlockJson {
                    address: format!("0x{address:04X}"),
                    hex: format_hex_line(&data),
                    decoded: decode_live_mode_block(address, &data),
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("Live mode begin id: {}", format_hex_line(&id));
        for (address, data) in blocks {
            println!("0x{address:04X}: {}", format_hex_line(&data));
            for line in decode_live_mode_block(address, &data) {
                println!("  - {line}");
            }
        }
        println!("Live mode end id: {}", format_hex_line(&end_id));
    }

    Ok(())
}

fn run_remote_live_write(args: RemoteLiveWriteArgs) -> Result<()> {
    let address = parse_u16_address(&args.address)?;
    let data = parse_exact_block_bytes(&args.bytes)?;
    validate_live_block_span(address, 1)?;
    if args.validate_only {
        println!(
            "Validated experimental live-mode EEPROM write to 0x{address:04X}: {}",
            format_hex_line(&data)
        );
        println!("No serial port was opened and no EEPROM write was attempted.");
        return Ok(());
    }
    if !args.yes {
        bail!(
            "remote live-write is experimental and writes live EEPROM. Re-run with --validate-only to preview, or pass --yes/--force to apply with readback verification."
        );
    }

    let port = resolve_port_for_args(&args.port)?;
    eprintln!(
        "WARNING: experimental protected live-mode EEPROM write to 0x{address:04X}; readback verification is {}.",
        if args.no_readback {
            "disabled"
        } else {
            "enabled"
        }
    );
    let (mut session, id) = LiveModeSession::begin(&port)?;
    println!("Live mode begin id: {}", format_hex_line(&id));

    let wrote = session.write_block(address, &data)?;
    if !wrote {
        bail!("Live-mode write to 0x{address:04X} was not acknowledged");
    }
    println!("Wrote 0x{address:04X}: {}", format_hex_line(&data));

    if !args.no_readback {
        let readback = session.read_block(address)?;
        println!("Readback 0x{address:04X}: {}", format_hex_line(&readback));
        if readback != data {
            let mismatch = readback
                .iter()
                .zip(data.iter())
                .enumerate()
                .find(|(_, (actual, expected))| actual != expected);
            if let Some((offset, (actual, expected))) = mismatch {
                bail!(
                    "Live-mode readback mismatch at 0x{:04X}: wrote {:02X}, read {:02X}",
                    address as usize + offset,
                    expected,
                    actual
                );
            }
            bail!("Live-mode readback mismatch after writing 0x{address:04X}");
        }
    }

    let end_id = session.close()?;
    println!("Live mode end id: {}", format_hex_line(&end_id));
    Ok(())
}

fn parse_u16_address(value: &str) -> Result<u16> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u16::from_str_radix(hex, 16)
            .map_err(|error| anyhow!("Invalid hex address '{trimmed}': {error}"))
    } else {
        trimmed
            .parse::<u16>()
            .map_err(|error| anyhow!("Invalid address '{trimmed}': {error}"))
    }
}

fn parse_exact_block_bytes(value: &str) -> Result<Vec<u8>> {
    let bytes = parse_remote_probe_bytes(Some(value))?;
    if bytes.len() != crate::protocol::BLOCK_SIZE {
        bail!(
            "Live-mode writes require exactly {} bytes, got {}",
            crate::protocol::BLOCK_SIZE,
            bytes.len()
        );
    }
    Ok(bytes)
}

fn format_hex_line(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

struct LiveModeSession {
    proto: RadioProtocol,
    closed: bool,
}

impl LiveModeSession {
    fn begin(port: &str) -> Result<(Self, [u8; 8])> {
        ensure_live_mode_supported(port)?;
        let mut proto = RadioProtocol::new(port)?;
        let id = match proto.live_mode_begin() {
            Ok(id) => id,
            Err(error) => {
                let _ = proto.live_mode_end();
                return Err(error);
            }
        };
        Ok((
            Self {
                proto,
                closed: false,
            },
            id,
        ))
    }

    fn read_block(&mut self, address: u16) -> Result<Vec<u8>> {
        self.proto.live_read_block(address)
    }

    fn write_block(&mut self, address: u16, data: &[u8]) -> Result<bool> {
        self.proto.live_write_block(address, data)
    }

    fn close(mut self) -> Result<[u8; 8]> {
        self.closed = true;
        self.proto.live_mode_end()
    }
}

impl Drop for LiveModeSession {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.proto.live_mode_end();
            self.closed = true;
        }
    }
}

fn validate_live_block_span(start_address: u16, blocks: u16) -> Result<()> {
    if blocks == 0 {
        bail!("Live-mode block count must be at least 1");
    }

    let start = start_address as usize;
    if !start.is_multiple_of(crate::protocol::BLOCK_SIZE) {
        bail!(
            "Live-mode address 0x{start_address:04X} must be aligned to {} bytes",
            crate::protocol::BLOCK_SIZE
        );
    }

    let byte_len = (blocks as usize)
        .checked_mul(crate::protocol::BLOCK_SIZE)
        .ok_or_else(|| anyhow!("Live-mode read size overflow for {blocks} blocks"))?;
    let end = start
        .checked_add(byte_len)
        .ok_or_else(|| anyhow!("Live-mode address range overflow"))?;
    if end > crate::protocol::EEPROM_SIZE {
        bail!(
            "Live-mode range 0x{start_address:04X}..0x{:04X} exceeds the {}-byte EEPROM window",
            end.saturating_sub(1),
            crate::protocol::EEPROM_SIZE
        );
    }

    Ok(())
}

fn decode_live_mode_block(address: u16, data: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    if data.len() != crate::protocol::BLOCK_SIZE {
        return lines;
    }

    match address {
        0x0CA0 => {
            let byte7 = data[7];
            let byte15 = data[15];
            lines.push(format!(
                "0x0CA7: tail-tone-cancel={} remote-kill={} remote-halo={} sound-control={}",
                bit(byte7, 6),
                bit(byte7, 4),
                bit(byte7, 3),
                byte7 & 0x07
            ));
            lines.push(format!(
                "0x0CAF: single-channel={} alarm-mode={}",
                bit(byte15, 7),
                if bit(byte15, 0) { "remote" } else { "local" }
            ));
        }
        0x0C80 => {
            lines.push(format!(
                "0x0C82/0x0C83 channel-modes: A={} B={}",
                decode_channel_mode(data[2] & 0x03),
                decode_channel_mode(data[3] & 0x03)
            ));
            lines.push(format!(
                "0x0C84/0x0C85/0x0C86 active-channels: A={} B={} RX={}",
                data[4], data[5], data[6]
            ));
            lines.push(format!(
                "0x0C90..0x0C92 short-keys: top={} side1={} side2={}",
                decode_key_code(data[16]),
                decode_key_code(data[17]),
                decode_key_code(data[18])
            ));
            lines.push(format!(
                "0x0C93..0x0C95 long-keys: top={} side1={} side2={}",
                decode_key_code(data[19]),
                decode_key_code(data[20]),
                decode_key_code(data[21])
            ));
        }
        0x1F20 => {
            lines.push(format!(
                "mic-gain={} raw={}",
                data[0],
                format_hex_line(data)
            ));
        }
        0x1F30 => {
            lines.push(format!("bluetooth-enabled={}", bit(data[0], 0)));
        }
        _ => {}
    }

    lines
}

fn bit(byte: u8, bit_index: u8) -> bool {
    byte & (1 << bit_index) != 0
}

fn decode_channel_mode(value: u8) -> &'static str {
    match value & 0x03 {
        0 => "frequency",
        1 => "memory",
        2 => "channel",
        _ => "unknown",
    }
}

fn decode_key_code(value: u8) -> &'static str {
    match value {
        0 => "none",
        1 => "radio",
        2 => "torch",
        3 => "cancel-squelch",
        4 => "tone",
        5 => "alarm",
        6 => "radio-alt",
        8 => "band",
        _ => "unknown",
    }
}

fn run_install_skill(args: InstallSkillArgs) -> Result<()> {
    let target = match args.agent {
        SkillAgentChoice::Auto => SkillInstallTarget::Auto,
        SkillAgentChoice::Codex => SkillInstallTarget::Codex,
        SkillAgentChoice::Claude => SkillInstallTarget::Claude,
        SkillAgentChoice::All => SkillInstallTarget::All,
    };

    let results = install_bundled_skill(target)?;
    for result in results {
        let status = if result.changed {
            "Installed"
        } else {
            "Already up to date"
        };
        println!(
            "{} NicTUI skill for {} at {}",
            status,
            result.agent.display_name(),
            result.skill_dir.display()
        );
    }
    println!("Inspect the bundled workflow with `nictui skill show`.");

    Ok(())
}

fn run_show_skill() {
    print!("{}", bundled_skill_markdown());
}

fn run_skill_paths(args: SkillPathsArgs) -> Result<()> {
    let detected = detected_agents();
    let agents = selected_skill_agents(args.agent, &detected);
    let views = agents
        .into_iter()
        .map(|agent| -> Result<SkillPathView> {
            Ok(SkillPathView {
                agent: agent.display_name().to_string(),
                detected: detected.contains(&agent),
                path: bundled_skill_dir(agent)?.display().to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if args.json {
        write_json_output(&views, None)?;
        return Ok(());
    }

    if detected.is_empty() && matches!(args.agent, SkillAgentChoice::Auto) {
        println!("No supported AI agents detected on PATH; showing default install locations.");
    }

    for view in views {
        println!(
            "{}{}: {}",
            view.agent,
            if view.detected { " [detected]" } else { "" },
            view.path
        );
    }

    Ok(())
}

fn selected_skill_agents(
    choice: SkillAgentChoice,
    detected: &[SupportedAgent],
) -> Vec<SupportedAgent> {
    match choice {
        SkillAgentChoice::Auto => {
            if detected.is_empty() {
                SupportedAgent::all().to_vec()
            } else {
                detected.to_vec()
            }
        }
        SkillAgentChoice::Codex => vec![SupportedAgent::Codex],
        SkillAgentChoice::Claude => vec![SupportedAgent::Claude],
        SkillAgentChoice::All => SupportedAgent::all().to_vec(),
    }
}

fn run_doctor(args: DoctorArgs) -> Result<()> {
    let output_dir = if let Some(dir) = args.output_dir {
        fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
        Some(dir)
    } else {
        None
    };
    let ble_readiness = explicit_ble_doctor_target(&args.port)
        .map(|(target, timeout)| assess_ble_readiness(Some(&target), timeout));

    if let Some(readiness) = ble_readiness.as_ref()
        && !readiness.ok
    {
        let report = DoctorReport {
            port: doctor_port_label(&args.port),
            handshake_ok: false,
            endian: None,
            channel_endian: None,
            firmware: None,
            live_mode: "not-evaluated".to_string(),
            remote_capability: "not-evaluated".to_string(),
            sections: vec![DoctorSection::failure("ble-readiness", readiness.detail())],
        };
        finalize_doctor_report(&report, output_dir.as_deref(), args.json)?;
        bail!("{}", readiness.summary);
    }

    let port = resolve_port_for_args(&args.port)?;
    let probe = probe_port(&port)?;
    let mut remote = assess_remote_capability(&port, &probe, None);

    let mut report = DoctorReport {
        port: port.clone(),
        handshake_ok: probe.handshake_ok,
        endian: probe.endian.map(format_endianness),
        channel_endian: probe.channel_endian.map(format_endianness),
        firmware: probe.firmware_variant.map(format_firmware_variant),
        live_mode: live_mode_capability(&probe).to_string(),
        remote_capability: remote.status.clone(),
        sections: Vec::new(),
    };

    if let Some(readiness) = ble_readiness.as_ref() {
        report.sections.push(DoctorSection::success(
            "ble-readiness",
            readiness.detail(),
            None,
        ));
    }

    if !probe.handshake_ok {
        report.sections.push(DoctorSection::success(
            "remote-capability",
            remote.detail,
            None,
        ));
        report.sections.push(DoctorSection::failure(
            "probe",
            "Handshake failed".to_string(),
        ));
        finalize_doctor_report(&report, output_dir.as_deref(), args.json)?;
        bail!("Handshake failed for {}", port);
    }

    if matches!(probe.firmware_variant, Some(FirmwareVariant::Stock)) {
        report.sections.push(DoctorSection::success(
            "remote-capability",
            remote.detail,
            None,
        ));
        report.sections.push(DoctorSection::failure(
            "firmware",
            "Radio appears to be running stock/original firmware. Install NicSure mod firmware before using NicTUI live read/write features.".to_string(),
        ));
        finalize_doctor_report(&report, output_dir.as_deref(), args.json)?;
        bail!(
            "{} appears to be running stock/original firmware. Install NicSure mod firmware before using NicTUI live read/write features.",
            port
        );
    }

    report.sections.push(DoctorSection::success(
        "probe",
        "Radio responded to handshake".to_string(),
        None,
    ));
    if let Some(hint) = live_mode_hint(&probe) {
        report
            .sections
            .push(DoctorSection::success("live-mode", hint, None));
    }

    if matches!(probe.firmware_variant, Some(FirmwareVariant::NicSure))
        && !port.starts_with("ble://")
    {
        let mut live_results = Vec::new();
        for gap_ms in [0u64, 50, 100] {
            live_results.push(run_pvojh_sweep_case(
                &port,
                PvojhSweepStage::StartId,
                gap_ms,
                50,
                800,
            )?);
        }
        let (live_mode_status, live_mode_detail) = summarize_pvojh_results(&live_results);
        report.live_mode = live_mode_status;
        remote = assess_remote_capability(&port, &probe, Some(report.live_mode.as_str()));
        report.remote_capability = remote.status.clone();

        let output = if let Some(dir) = output_dir.as_deref() {
            let path = dir.join("live-mode-check.json");
            write_json_output(&live_results, Some(path.as_path()))?;
            Some(path)
        } else {
            None
        };
        report.sections.push(DoctorSection::success(
            "live-mode-check",
            live_mode_detail,
            output,
        ));
    }

    report.sections.push(DoctorSection::success(
        "remote-capability",
        remote.detail,
        None,
    ));

    let mut had_failures = false;

    let mut channels_progress = ProgressPrinter::default();
    match read_channels(&port, |event| channels_progress.handle(event)) {
        Ok((channels, _)) => {
            let output = if let Some(dir) = output_dir.as_deref() {
                let path = dir.join("channels.json");
                write_channels_output(&channels, Some(path.as_path()), ChannelOutputFormat::Json)?;
                Some(path)
            } else {
                None
            };
            report.sections.push(DoctorSection::success(
                "channels",
                format!("{} channels", channels.len()),
                output,
            ));
        }
        Err(error) => {
            had_failures = true;
            report
                .sections
                .push(DoctorSection::failure("channels", error.to_string()));
        }
    }

    let mut settings_progress = ProgressPrinter::default();
    match read_settings(&port, |event| settings_progress.handle(event)) {
        Ok((settings, _)) => {
            let output = if let Some(dir) = output_dir.as_deref() {
                let path = dir.join("settings.json");
                write_json_output(&settings, Some(path.as_path()))?;
                Some(path)
            } else {
                None
            };
            report.sections.push(DoctorSection::success(
                "settings",
                "Settings block read successfully".to_string(),
                output,
            ));
        }
        Err(error) => {
            had_failures = true;
            report
                .sections
                .push(DoctorSection::failure("settings", error.to_string()));
        }
    }

    let mut scan_progress = ProgressPrinter::default();
    match read_scan_presets(&port, |event| scan_progress.handle(event)) {
        Ok((scan_presets, _)) => {
            let output = if let Some(dir) = output_dir.as_deref() {
                let path = dir.join("scan-presets.json");
                write_json_output(&scan_presets, Some(path.as_path()))?;
                Some(path)
            } else {
                None
            };
            report.sections.push(DoctorSection::success(
                "scan-presets",
                format!("{} presets", scan_presets.len()),
                output,
            ));
        }
        Err(error) => {
            had_failures = true;
            report
                .sections
                .push(DoctorSection::failure("scan-presets", error.to_string()));
        }
    }

    let mut band_plan_progress = ProgressPrinter::default();
    match read_band_plans(&port, |event| band_plan_progress.handle(event)) {
        Ok((band_plans, _)) => {
            let output = if let Some(dir) = output_dir.as_deref() {
                let path = dir.join("band-plan.json");
                write_json_output(&band_plans, Some(path.as_path()))?;
                Some(path)
            } else {
                None
            };
            report.sections.push(DoctorSection::success(
                "band-plan",
                format!("{} band plans", band_plans.len()),
                output,
            ));
        }
        Err(error) => {
            had_failures = true;
            report
                .sections
                .push(DoctorSection::failure("band-plan", error.to_string()));
        }
    }

    let mut dtmf_progress = ProgressPrinter::default();
    match read_dtmf_presets(&port, |event| dtmf_progress.handle(event)) {
        Ok(dtmf_presets) => {
            let output = if let Some(dir) = output_dir.as_deref() {
                let path = dir.join("dtmf.json");
                write_json_output(&dtmf_presets, Some(path.as_path()))?;
                Some(path)
            } else {
                None
            };
            report.sections.push(DoctorSection::success(
                "dtmf",
                format!("{} presets", dtmf_presets.len()),
                output,
            ));
        }
        Err(error) => {
            had_failures = true;
            report
                .sections
                .push(DoctorSection::failure("dtmf", error.to_string()));
        }
    }

    if args.codeplug {
        let mut codeplug_progress = ProgressPrinter::default();
        match read_codeplug(&port, |event| codeplug_progress.handle(event)) {
            Ok((codeplug, _)) => {
                let inspection = inspect_codeplug(&codeplug)?;
                let output = if let Some(dir) = output_dir.as_deref() {
                    let codeplug_path = dir.join("radio.nfw");
                    save_codeplug(&codeplug_path, &codeplug)?;

                    let inspection_path = dir.join("codeplug-inspection.json");
                    write_json_output(&inspection, Some(inspection_path.as_path()))?;

                    Some(codeplug_path)
                } else {
                    None
                };
                report.sections.push(DoctorSection::success(
                    "codeplug",
                    format!(
                        "{} bytes, {} VFO memories, {} channels, {} scan presets, {} band plans, {} dtmf presets, {} unknown regions ({} with live data)",
                        inspection.size,
                        inspection.vfo_memory_count,
                        inspection.channel_count,
                        inspection.scan_preset_count,
                        inspection.band_plan_count,
                        inspection.dtmf_preset_count,
                        inspection.unknown_region_count,
                        inspection.unknown_regions_with_live_data
                    ),
                    output,
                ));
            }
            Err(error) => {
                had_failures = true;
                report
                    .sections
                    .push(DoctorSection::failure("codeplug", error.to_string()));
            }
        }
    }

    finalize_doctor_report(&report, output_dir.as_deref(), args.json)?;

    if had_failures {
        bail!("Doctor found one or more failures")
    }

    Ok(())
}

impl RemoteKey {
    fn as_key_code(self) -> u8 {
        match self {
            RemoteKey::Digit0 => 0x01,
            RemoteKey::Digit1 => 0x02,
            RemoteKey::Digit2 => 0x03,
            RemoteKey::Digit3 => 0x04,
            RemoteKey::Digit4 => 0x05,
            RemoteKey::Digit5 => 0x06,
            RemoteKey::Digit6 => 0x07,
            RemoteKey::Digit7 => 0x08,
            RemoteKey::Digit8 => 0x09,
            RemoteKey::Digit9 => 0x0A,
            RemoteKey::Menu => 0x0B,
            RemoteKey::Up => 0x0C,
            RemoteKey::Down => 0x0D,
            RemoteKey::Exit => 0x0E,
            RemoteKey::Star => 0x0F,
            RemoteKey::Pound => 0x10,
            RemoteKey::PttA => 0x13,
            RemoteKey::PttB => 0x1A,
            RemoteKey::Flashlight => 0x12,
            RemoteKey::Vm => 0x11,
        }
    }

    fn as_command(self) -> RemoteControlCommand {
        RemoteControlCommand::raw_key(format!("{self:?}").to_lowercase(), self.as_key_code())
    }
}

impl RemoteProbePreset {
    fn as_command(self, repeat: u32, gap_ms: u64, hold_ms: u64) -> RemoteControlCommand {
        match self {
            RemoteProbePreset::Menu => RemoteControlCommand::raw_key("menu", 0x0B),
            RemoteProbePreset::Up => RemoteControlCommand::raw_key("up", 0x0C),
            RemoteProbePreset::Down => RemoteControlCommand::raw_key("down", 0x0D),
            RemoteProbePreset::Exit => RemoteControlCommand::raw_key("exit", 0x0E),
            RemoteProbePreset::PttA => RemoteControlCommand::raw_key("ptt-a", 0x13),
            RemoteProbePreset::PttB => RemoteControlCommand::raw_key("ptt-b", 0x1A),
            RemoteProbePreset::Flashlight => RemoteControlCommand::raw_key("flashlight", 0x12),
            RemoteProbePreset::Vm => RemoteControlCommand::raw_key("v/m", 0x11),
            RemoteProbePreset::HoldMenu => RemoteControlCommand::held_key(
                "hold-menu",
                0x0B,
                0x00,
                std::time::Duration::from_millis(gap_ms),
                repeat.max(1),
                std::time::Duration::from_millis(hold_ms.max(250)),
            ),
            RemoteProbePreset::HoldPttA => RemoteControlCommand::held_key(
                "hold-ptt-a",
                0x13,
                0x00,
                std::time::Duration::from_millis(gap_ms),
                repeat.max(1),
                std::time::Duration::from_millis(hold_ms.max(250)),
            ),
            RemoteProbePreset::TelemetryPrime => telemetry_prime_command(),
        }
    }
}

fn telemetry_prime_command() -> RemoteControlCommand {
    RemoteControlCommand::burst(
        "telemetry-prime",
        vec![
            0x64, 0x00, 0x67, 0x46, 0x9A, 0xFE, 0x00, 0x00, 0x20, 0x36, 0x31, 0x25,
        ],
        Duration::from_millis(700),
    )
}

fn write_channels_output(
    channels: &[crate::protocol::Channel],
    output: Option<&Path>,
    format: ChannelOutputFormat,
) -> Result<()> {
    match output {
        Some(path) => {
            let writer = BufWriter::new(
                File::create(path)
                    .with_context(|| format!("Failed to create {}", path.display()))?,
            );
            save_channels_to_writer(writer, channels, format.into())
        }
        None => {
            let stdout = io::stdout();
            let handle = stdout.lock();
            save_channels_to_writer(handle, channels, format.into())?;
            if matches!(format, ChannelOutputFormat::Json) {
                println!();
            }
            Ok(())
        }
    }
}

fn write_single_channel_output(
    channel: &Channel,
    output: Option<&Path>,
    format: Option<ChannelOutputFormat>,
) -> Result<()> {
    let format = format.unwrap_or_else(|| match output {
        Some(path) => match infer_channel_file_format(path) {
            Ok(ChannelFileFormat::Csv) => ChannelOutputFormat::Csv,
            _ => ChannelOutputFormat::Json,
        },
        None => ChannelOutputFormat::Json,
    });

    match format {
        ChannelOutputFormat::Csv => write_channels_output(
            std::slice::from_ref(channel),
            output,
            ChannelOutputFormat::Csv,
        ),
        ChannelOutputFormat::Json => write_json_output(channel, output),
    }
}

fn write_json_output<T: Serialize>(value: &T, output: Option<&Path>) -> Result<()> {
    match output {
        Some(path) => {
            let writer = BufWriter::new(
                File::create(path)
                    .with_context(|| format!("Failed to create {}", path.display()))?,
            );
            serde_json::to_writer_pretty(writer, value)
                .with_context(|| format!("Failed to write {}", path.display()))
        }
        None => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer_pretty(&mut handle, value).context("Failed to write JSON")?;
            writeln!(handle).context("Failed to terminate JSON output")
        }
    }
}

fn read_json_input<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let reader = BufReader::new(
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?,
    );
    serde_json::from_reader(reader).with_context(|| format!("Failed to parse {}", path.display()))
}

fn read_single_json_or_array_input<T: DeserializeOwned>(path: &Path, label: &str) -> Result<T> {
    let reader = BufReader::new(
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?,
    );
    let value: serde_json::Value = serde_json::from_reader(reader)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    if value.is_array() {
        let mut items: Vec<T> = serde_json::from_value(value)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        if items.len() != 1 {
            bail!(
                "Expected exactly one {} in {}, found {}.",
                label,
                path.display(),
                items.len()
            );
        }
        Ok(items.remove(0))
    } else {
        serde_json::from_value(value).with_context(|| format!("Failed to parse {}", path.display()))
    }
}

fn load_single_channel_from_path(path: &Path) -> Result<Channel> {
    match infer_channel_file_format(path)? {
        ChannelFileFormat::Csv => {
            let mut channels = load_channels_from_path(path)?;
            if channels.len() != 1 {
                bail!(
                    "Expected exactly one channel in {}, found {}.",
                    path.display(),
                    channels.len()
                );
            }
            Ok(channels.remove(0))
        }
        ChannelFileFormat::Json => read_single_json_or_array_input(path, "channel"),
    }
}

fn validate_channel_number(channel: u16) -> Result<()> {
    if !(1..=198).contains(&channel) {
        bail!(
            "Channel {} is out of range. Valid channel numbers are 1-198.",
            channel
        );
    }
    Ok(())
}

fn validate_channel_range(start: u16, end: u16) -> Result<Vec<u16>> {
    validate_channel_number(start)?;
    validate_channel_number(end)?;
    if start > end {
        bail!(
            "Channel range {}-{} is invalid. Start must be less than or equal to end.",
            start,
            end
        );
    }
    Ok((start..=end).collect())
}

fn validate_group_number(group: u8) -> Result<usize> {
    if !(1..=crate::protocol::GROUP_LABEL_COUNT as u8).contains(&group) {
        bail!(
            "Group {} is out of range. Valid group numbers are 1-{}.",
            group,
            crate::protocol::GROUP_LABEL_COUNT
        );
    }
    Ok((group - 1) as usize)
}

fn normalize_group_label(label: &str) -> String {
    label
        .trim()
        .chars()
        .take(crate::protocol::GROUP_LABEL_SIZE)
        .collect()
}

fn bluetooth_setting_index() -> usize {
    resolve_setting_selector("Bluetooth").expect("Bluetooth setting metadata missing")
}

fn ensure_index_in_range(index: u8, max_len: usize, label: &str) -> Result<()> {
    if index as usize >= max_len {
        bail!(
            "{} index {} is out of range. Valid indices are 0-{}.",
            label,
            index,
            max_len.saturating_sub(1)
        );
    }
    Ok(())
}

fn resolve_setting_selector(selector: &str) -> Result<usize> {
    let trimmed = selector.trim();
    if let Some((index, _)) = SETTINGS_METADATA
        .iter()
        .enumerate()
        .find(|(_, meta)| meta.menu_num.eq_ignore_ascii_case(trimmed))
    {
        return Ok(index);
    }

    if let Ok(index) = trimmed.parse::<usize>()
        && index < SETTINGS_METADATA.len()
    {
        return Ok(index);
    }

    let normalized = normalize_setting_selector(trimmed);
    let mut matches = SETTINGS_METADATA
        .iter()
        .enumerate()
        .filter(|(_, meta)| normalize_setting_selector(meta.name) == normalized)
        .map(|(index, _)| index);

    match (matches.next(), matches.next()) {
        (Some(index), None) => Ok(index),
        (Some(_), Some(_)) => bail!("Setting selector '{}' is ambiguous.", selector),
        _ => bail!(
            "Unknown setting '{}'. Use a menu number like 17 or a setting name like \"LCD Brightness\".",
            selector
        ),
    }
}

fn normalize_setting_selector(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn parse_setting_value(index: usize, value: &str) -> Result<u32> {
    let meta = &SETTINGS_METADATA[index];
    match meta.setting_type {
        SettingType::Boolean => parse_bool_setting_value(value),
        SettingType::Enum(options) => parse_enum_setting_value(meta.name, options, value),
        SettingType::Numeric { min, max, .. } => {
            let parsed = value
                .parse::<i32>()
                .with_context(|| format!("Setting '{}' expects a numeric value.", meta.name))?;
            if !(min..=max).contains(&parsed) {
                bail!(
                    "Setting '{}' is out of range. Valid values are {}-{}.",
                    meta.name,
                    min,
                    max
                );
            }
            Ok(parsed as u32)
        }
    }
}

fn parse_bool_setting_value(value: &str) -> Result<u32> {
    let normalized = normalize_setting_selector(value);
    match normalized.as_str() {
        "1" | "on" | "true" | "yes" | "enabled" => Ok(1),
        "0" | "off" | "false" | "no" | "disabled" => Ok(0),
        _ => bail!(
            "Boolean settings accept on/off, true/false, yes/no, or 1/0. Got '{}'.",
            value
        ),
    }
}

fn parse_enum_setting_value(
    setting_name: &str,
    options: &[&'static str],
    value: &str,
) -> Result<u32> {
    if let Ok(index) = value.parse::<usize>()
        && index < options.len()
    {
        return Ok(index as u32);
    }

    let normalized = normalize_setting_selector(value);
    if let Some((index, _)) = options
        .iter()
        .enumerate()
        .find(|(_, option)| normalize_setting_selector(option) == normalized)
    {
        return Ok(index as u32);
    }

    bail!(
        "Setting '{}' accepts one of: {}.",
        setting_name,
        options.join(", ")
    )
}

#[derive(Serialize)]
struct SettingView {
    index: usize,
    menu_num: &'static str,
    name: &'static str,
    kind: &'static str,
    raw_value: u32,
    display_value: String,
    min: Option<i32>,
    max: Option<i32>,
    unit: Option<&'static str>,
    options: Option<Vec<&'static str>>,
}

fn build_setting_view(settings: &SettingsBlock, index: usize) -> SettingView {
    let meta = &SETTINGS_METADATA[index];
    let raw_value = settings.get_value(index);
    let display_value = settings.get_display_value(index);
    let (kind, min, max, unit, options) = match meta.setting_type {
        SettingType::Boolean => ("boolean", None, None, None, Some(vec!["Off", "On"])),
        SettingType::Enum(values) => ("enum", None, None, None, Some(values.to_vec())),
        SettingType::Numeric { min, max, unit } => (
            "numeric",
            Some(min),
            Some(max),
            if unit.is_empty() { None } else { Some(unit) },
            None,
        ),
    };

    SettingView {
        index,
        menu_num: meta.menu_num,
        name: meta.name,
        kind,
        raw_value,
        display_value,
        min,
        max,
        unit,
        options,
    }
}

fn validate_settings_payload(settings: &SettingsBlock) -> Result<()> {
    let _ = RadioProtocol::pack_settings_block(settings, Endianness::Little);
    let _ = RadioProtocol::pack_settings_block(settings, Endianness::Big);
    Ok(())
}

fn validate_firmware_image(path: &Path, firmware: &[u8]) -> Result<()> {
    let rounded_len = firmware.len().div_ceil(32) * 32;
    if rounded_len > 0xF800 {
        bail!(
            "Firmware {} is too large: {} bytes rounded to {} bytes exceeds the radio limit of {} bytes.",
            path.display(),
            firmware.len(),
            rounded_len,
            0xF800
        );
    }

    let block_count = rounded_len / 32;
    println!(
        "Validated firmware image {}: {} bytes ({} blocks on wire)",
        path.display(),
        firmware.len(),
        block_count
    );
    Ok(())
}

fn print_channel_validation_summary(path: &Path, channels: &[Channel]) {
    let active = channels
        .iter()
        .filter(|channel| channel.position == 1)
        .count();
    let inactive = channels.len().saturating_sub(active);
    println!(
        "Validated channel file {}: {} channels ({} active, {} parked)",
        path.display(),
        channels.len(),
        active,
        inactive
    );
}

fn print_targeted_channel_validation_summary(path: &Path, channel: &Channel) {
    println!(
        "Validated channel update {}: slot {} ready to write ({})",
        path.display(),
        channel.channel_num,
        if channel.name.is_empty() {
            "<unnamed>"
        } else {
            channel.name.as_str()
        }
    );
}

fn print_settings_validation_summary(path: &Path, settings: &SettingsBlock) {
    println!(
        "Validated settings file {}: {} editable fields ready to write (magic 0x{:04X})",
        path.display(),
        SETTINGS_METADATA.len(),
        settings.magic
    );
}

fn print_setting_change_validation_summary(index: usize, raw_value: u32) {
    let meta = &SETTINGS_METADATA[index];
    println!(
        "Validated setting change M{} {} -> raw {}",
        meta.menu_num, meta.name, raw_value
    );
}

fn print_scan_preset_validation_summary(path: &Path, scan_presets: &[ScanPreset]) {
    println!(
        "Validated scan preset file {}: {} presets ready to write",
        path.display(),
        scan_presets.len()
    );
}

fn print_band_plan_validation_summary(path: &Path, band_plans: &[BandPlan]) {
    println!(
        "Validated band plan file {}: {} band plans ready to write",
        path.display(),
        band_plans.len()
    );
}

fn print_dtmf_validation_summary(path: &Path, dtmf_presets: &[DTMFPreset]) {
    println!(
        "Validated DTMF file {}: {} presets ready to write",
        path.display(),
        dtmf_presets.len()
    );
}

fn print_targeted_index_validation_summary(label: &str, index: u8, path: &Path) {
    println!(
        "Validated {} update {} from {}",
        label,
        index,
        path.display()
    );
}

fn print_codeplug_validation_summary(path: &Path, inspection: &crate::device::CodeplugInspection) {
    println!(
        "Validated codeplug {}: {} bytes, {} VFO memories, {} channels, {} scan presets, {} band plans, {} DTMF presets, {} named groups, {} unknown regions ({} with live data)",
        path.display(),
        inspection.size,
        inspection.vfo_memory_count,
        inspection.channel_count,
        inspection.scan_preset_count,
        inspection.band_plan_count,
        inspection.dtmf_preset_count,
        inspection
            .group_labels
            .iter()
            .filter(|label| !label.trim().is_empty())
            .count(),
        inspection.unknown_region_count,
        inspection.unknown_regions_with_live_data
    );
}

impl From<ChannelOutputFormat> for ChannelFileFormat {
    fn from(value: ChannelOutputFormat) -> Self {
        match value {
            ChannelOutputFormat::Csv => ChannelFileFormat::Csv,
            ChannelOutputFormat::Json => ChannelFileFormat::Json,
        }
    }
}

fn detect_radio_endianness(port: &str) -> Result<crate::protocol::Endianness> {
    let probe = probe_port(port)?;
    ensure_probe_supports_live_mode(port, &probe)?;
    Ok(probe.endian.unwrap_or(crate::protocol::Endianness::Big))
}

fn ensure_live_mode_supported(port: &str) -> Result<()> {
    let probe = probe_port(port)?;
    ensure_probe_supports_live_mode(port, &probe)
}

fn ensure_probe_supports_live_mode(port: &str, probe: &crate::device::ProbeResult) -> Result<()> {
    if !probe.handshake_ok {
        bail!("Handshake failed for {}", port);
    }
    if matches!(probe.firmware_variant, Some(FirmwareVariant::Stock)) {
        bail!(
            "{} appears to be running stock/original firmware. Install NicSure mod firmware before using NicTUI live read/write features.",
            port
        );
    }
    Ok(())
}

fn resolve_optional_port_for_args(args: &PortArgs) -> Result<Option<String>> {
    if args.port.is_none() && args.ble_device.is_none() && args.ble_name.is_none() {
        return Ok(None);
    }

    resolve_port_for_args(args).map(Some)
}

fn resolve_port_for_args(args: &PortArgs) -> Result<String> {
    if let Some(port) = args.port.as_deref()
        && let Some(device_id) = port
            .strip_prefix("ble://")
            .or_else(|| port.strip_prefix("ble:"))
    {
        return bridge_ble_target(
            BleTarget::Device(device_id.trim().to_string()),
            default_scan_timeout(),
        );
    }

    if let Some(device_id) = args.ble_device.as_deref() {
        return bridge_ble_target(
            BleTarget::Device(device_id.trim().to_string()),
            default_scan_timeout(),
        );
    }

    if let Some(name) = args.ble_name.as_deref() {
        return bridge_ble_target(
            BleTarget::Name(name.trim().to_string()),
            Duration::from_secs(args.ble_scan_time.max(1)),
        );
    }

    let port = resolve_port(args.port.as_deref())?;
    if args.port.is_none() && list_ports()?.len() > 1 {
        eprintln!("Auto-detected radio port: {}", port);
    }
    Ok(port)
}

fn bridge_ble_target(target: BleTarget, timeout: Duration) -> Result<String> {
    let bridge = ensure_ble_bridge(&target, timeout)?;
    Ok(bridge.tty_path)
}

fn optional_ble_target_from_options(
    device: Option<&str>,
    name: Option<&str>,
) -> Result<Option<BleTarget>> {
    match (device.map(str::trim), name.map(str::trim)) {
        (None, None) => Ok(None),
        (Some(device), None) if !device.is_empty() => {
            Ok(Some(BleTarget::Device(device.to_string())))
        }
        (None, Some(name)) if !name.is_empty() => Ok(Some(BleTarget::Name(name.to_string()))),
        _ => bail!("Pass at most one of --device or --name."),
    }
}

fn ble_target_from_options(device: Option<&str>, name: Option<&str>) -> Result<BleTarget> {
    optional_ble_target_from_options(device, name)?
        .ok_or_else(|| anyhow!("Pass exactly one of --device or --name."))
}

fn explicit_ble_doctor_target(args: &PortArgs) -> Option<(BleTarget, Duration)> {
    if let Some(port) = args.port.as_deref()
        && let Some(device_id) = parse_ble_device_uri(port)
    {
        return Some((
            BleTarget::Device(device_id.trim().to_string()),
            default_scan_timeout(),
        ));
    }

    if let Some(device_id) = args.ble_device.as_deref() {
        return Some((
            BleTarget::Device(device_id.trim().to_string()),
            default_scan_timeout(),
        ));
    }

    args.ble_name.as_deref().map(|name| {
        (
            BleTarget::Name(name.trim().to_string()),
            Duration::from_secs(args.ble_scan_time.max(1)),
        )
    })
}

fn doctor_port_label(args: &PortArgs) -> String {
    if let Some(port) = args.port.as_deref() {
        return port.to_string();
    }

    if let Some(device_id) = args.ble_device.as_deref() {
        return format!("ble://{}", device_id.trim());
    }

    if let Some(name) = args.ble_name.as_deref() {
        return format!("ble-name:{}", name.trim());
    }

    "<auto>".to_string()
}

fn print_ble_readiness_report(report: &BleReadinessReport) {
    println!("BLE readiness: {}", report.kind);
    println!("Stage: {}", report.stage);
    println!("Status: {}", if report.ok { "ok" } else { "failed" });
    println!("Summary: {}", report.summary);
    println!("Next action: {}", report.next_action);

    if let Some(target) = &report.target {
        println!("Target: {}", target);
    }

    if let Some(device) = &report.device {
        println!("Device: {}", device.device_id);
        if let Some(name) = &device.name {
            println!("Name: {}", name);
        }
        if let Some(rssi) = device.rssi {
            println!("RSSI: {}", rssi);
        }
    }
}

fn format_endianness(endian: crate::protocol::Endianness) -> String {
    format!("{endian:?}")
}

fn format_firmware_variant(firmware: FirmwareVariant) -> String {
    firmware.to_string()
}

fn format_region_offsets(start_offset: usize, end_offset_exclusive: usize) -> String {
    format!("0x{start_offset:04X}..0x{end_offset_exclusive:04X}")
}

fn format_region_size(length: usize) -> String {
    format!("{length} bytes")
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothCommand, ChannelsCommand, Cli, Commands, PvojhSweepResult, PvojhSweepStage,
        RemoteCommand, RemoteDiagnoseCaseView, RemoteDiagnoseControlView, RemoteProbePreset,
        SkillCommand, assess_remote_capability, classify_remote_diagnose, decode_live_mode_block,
        doctor_port_label, explicit_ble_doctor_target, normalize_group_label, parse_u16_address,
        parse_u64_list, probe_view, summarize_pvojh_results, summarize_remote_diagnose,
        summarize_remote_key_send_report, validate_channel_range, validate_group_number,
        validate_live_block_span,
    };
    use crate::device::{FirmwareVariant, ProbeResult, RemoteKeySendReport};
    use crate::remote::{
        RemoteCommandReaction, RemoteControlReport, RemoteControlStrategy, RemoteEvidenceKind,
    };
    use clap::{CommandFactory, Parser, ValueEnum};

    #[test]
    fn validate_channel_range_accepts_inclusive_range() {
        assert_eq!(validate_channel_range(8, 10).unwrap(), vec![8, 9, 10]);
    }

    #[test]
    fn validate_channel_range_rejects_reversed_range() {
        assert!(validate_channel_range(10, 8).is_err());
    }

    #[test]
    fn validate_group_number_accepts_one_based_groups() {
        assert_eq!(validate_group_number(1).unwrap(), 0);
        assert_eq!(validate_group_number(16).unwrap(), 15);
    }

    #[test]
    fn normalize_group_label_trims_and_truncates() {
        assert_eq!(normalize_group_label("  WEATHER  "), "WEATHE");
    }

    #[test]
    fn bluetooth_alias_parses_status_command() {
        let cli = Cli::parse_from(["nictui", "ble", "status"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Bluetooth {
                command: BluetoothCommand::Status(_)
            })
        ));
    }

    #[test]
    fn bluetooth_connect_accepts_name_target() {
        let cli = Cli::parse_from(["nictui", "bluetooth", "connect", "--name", "TD-H3"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Bluetooth {
                command: BluetoothCommand::Connect(_)
            })
        ));
    }

    #[test]
    fn bluetooth_doctor_accepts_optional_target() {
        let cli = Cli::parse_from(["nictui", "bluetooth", "doctor", "--device", "abc"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Bluetooth {
                command: BluetoothCommand::Doctor(_)
            })
        ));
    }

    #[test]
    fn shared_port_args_accept_ble_device() {
        let cli = Cli::parse_from([
            "nictui",
            "channels",
            "read",
            "--ble-device",
            "12345678-1234-5678-9ABC-DEF012345678",
        ]);
        assert!(matches!(
            cli.command,
            Some(Commands::Channels {
                command: ChannelsCommand::Read(_)
            })
        ));
    }

    #[test]
    fn explicit_ble_doctor_target_prefers_ble_name_timeout() {
        let cli = Cli::parse_from([
            "nictui",
            "doctor",
            "--ble-name",
            "TD-H3",
            "--ble-scan-time",
            "12",
        ]);
        let Some(Commands::Doctor(args)) = cli.command else {
            panic!("doctor args missing");
        };
        let (_, timeout) = explicit_ble_doctor_target(&args.port).expect("ble target");
        assert_eq!(timeout, std::time::Duration::from_secs(12));
    }

    #[test]
    fn doctor_port_label_renders_ble_name_requests() {
        let cli = Cli::parse_from(["nictui", "doctor", "--ble-name", "TD-H3"]);
        let Some(Commands::Doctor(args)) = cli.command else {
            panic!("doctor args missing");
        };
        assert_eq!(doctor_port_label(&args.port), "ble-name:TD-H3");
    }

    #[test]
    fn clap_command_tree_debug_asserts() {
        Cli::command().debug_assert();
    }

    #[test]
    fn help_renders_for_every_command() {
        fn render_all(command: clap::Command) {
            let mut command = command;
            let _ = command.render_help().to_string();
            let subcommands = command.get_subcommands().cloned().collect::<Vec<_>>();
            for subcommand in subcommands {
                render_all(subcommand);
            }
        }

        render_all(Cli::command());
    }

    #[test]
    fn release_help_surfaces_safe_workflow_and_evidence_limits() {
        let mut command = Cli::command();
        let help = command.render_help().to_string();

        assert!(help.contains("Safe release workflow"));
        assert!(help.contains("bridge readiness does not prove"));
        assert!(help.contains("remote_control_confirmed: true"));
    }

    #[test]
    fn remote_help_requires_decoded_delta_for_confirmed_control() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("remote")
            .expect("remote command")
            .render_help()
            .to_string();

        assert!(help.contains("Remote evidence notes"));
        assert!(help.contains("Confirmed control requires a decoded state delta"));
        assert!(help.contains("nictui remote diagnose --port <serial> --json"));
    }

    #[test]
    fn bluetooth_help_distinguishes_bridge_from_control() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("bluetooth")
            .expect("bluetooth command")
            .render_help()
            .to_string();

        assert!(help.contains("BLE workflow"));
        assert!(help.contains("open the BLE transport"));
        assert!(help.contains("It does not"));
        assert!(help.contains("prove remote control"));
    }

    #[test]
    fn write_help_documents_reboot_flag() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("channels")
            .expect("channels command")
            .find_subcommand_mut("write")
            .expect("channels write command")
            .render_help()
            .to_string();

        assert!(help.contains("Reboot the radio after writing channels"));
    }

    #[test]
    fn settings_write_help_documents_no_reboot_flag() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("settings")
            .expect("settings command")
            .find_subcommand_mut("write")
            .expect("settings write command")
            .render_help()
            .to_string();

        assert!(help.contains("Skip the reboot that normally follows a settings write"));
        let cli = Cli::try_parse_from([
            "nictui",
            "settings",
            "write",
            "--input",
            "settings.json",
            "--no-reboot",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Settings {
                command: super::SettingsCommand::Write(_)
            })
        ));
    }

    #[test]
    fn codeplug_write_help_documents_no_reboot_flag() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("codeplug")
            .expect("codeplug command")
            .find_subcommand_mut("write")
            .expect("codeplug write command")
            .render_help()
            .to_string();

        assert!(help.contains("Skip the reboot that normally follows a codeplug write"));
        assert!(
            Cli::try_parse_from([
                "nictui",
                "codeplug",
                "write",
                "--input",
                "radio.nfw",
                "--no-reboot"
            ])
            .is_ok()
        );
    }

    #[test]
    fn command_aliases_parse_to_expected_variants() {
        let cli = Cli::try_parse_from(["nictui", "channels", "show", "--channel", "25"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Channels {
                command: ChannelsCommand::Get(_)
            })
        ));

        let cli = Cli::try_parse_from(["nictui", "skill", "sync", "--agent", "all"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Skill {
                command: SkillCommand::Install(_)
            })
        ));

        let cli = Cli::try_parse_from(["nictui", "scan", "show", "--index", "1"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::ScanPresets { .. })));
    }

    #[test]
    fn remote_probe_presets_expand_to_commands() {
        let hold_menu = RemoteProbePreset::HoldMenu.as_command(2, 80, 500);
        assert_eq!(hold_menu.bytes_hex(), "8A FF 8A FF");
        assert_eq!(
            hold_menu.steps[0].pause_after,
            std::time::Duration::from_millis(500)
        );

        let ptt_a = RemoteProbePreset::PttA.as_command(1, 80, 0);
        assert_eq!(ptt_a.bytes_hex(), "90 FF");

        let telemetry_prime = RemoteProbePreset::TelemetryPrime.as_command(1, 80, 0);
        assert_eq!(
            telemetry_prime.bytes_hex(),
            "64 00 67 46 9A FE 00 00 20 36 31 25"
        );
        assert_eq!(telemetry_prime.steps.len(), 1);
    }

    #[test]
    fn remote_matrix_wraps_probe_with_sync_steps() {
        let base = RemoteProbePreset::Menu.as_command(1, 80, 0);
        let matrix = super::remote_matrix_scenarios(&base, 80);

        assert_eq!(matrix.len(), 6);
        assert_eq!(matrix[2].command.bytes_hex(), "4A 8A FF");
        assert_eq!(matrix[4].command.bytes_hex(), "4A 8A FF 4A");
    }

    #[test]
    fn remote_probe_raw_bytes_remain_literal_wire_bytes() {
        let command = super::build_remote_probe_command(None, Some("0B,00"), 2, 80, 0).unwrap();

        assert_eq!(command.strategy, RemoteControlStrategy::Sequence);
        assert_eq!(command.bytes_hex(), "0B 00 0B 00");
    }

    #[test]
    fn remote_live_read_command_parses_hex_address() {
        let cli =
            Cli::try_parse_from(["nictui", "remote", "live-read", "--address", "0x0CA0"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Remote {
                command: RemoteCommand::LiveRead(_)
            })
        ));
        assert_eq!(parse_u16_address("0x0CA0").unwrap(), 0x0CA0);
    }

    #[test]
    fn remote_live_write_supports_validate_only_and_force_alias() {
        let bytes = "00 ".repeat(crate::protocol::BLOCK_SIZE);
        let cli = Cli::try_parse_from([
            "nictui",
            "remote",
            "live-write",
            "--address",
            "0x0CA0",
            "--bytes",
            bytes.trim(),
            "--validate-only",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Remote {
                command: RemoteCommand::LiveWrite(_)
            })
        ));

        let cli = Cli::try_parse_from([
            "nictui",
            "remote",
            "live-write",
            "--address",
            "0x0CA0",
            "--bytes",
            bytes.trim(),
            "--force",
            "--no-readback",
        ])
        .unwrap();
        let Some(Commands::Remote {
            command: RemoteCommand::LiveWrite(args),
        }) = cli.command
        else {
            panic!("live-write args missing");
        };
        assert!(args.yes);
        assert!(args.no_readback);
    }

    #[test]
    fn remote_pvojh_sweep_command_parses_gap_list() {
        let cli = Cli::try_parse_from([
            "nictui",
            "remote",
            "pvojh-sweep",
            "--stage",
            "start-id",
            "--gap-ms",
            "0,20,80",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Remote {
                command: RemoteCommand::PvojhSweep(_)
            })
        ));
        assert_eq!(parse_u64_list("0,20,80").unwrap(), vec![0, 20, 80]);
        assert_eq!(
            PvojhSweepStage::StartId
                .to_possible_value()
                .unwrap()
                .get_name(),
            "start-id"
        );
    }

    #[test]
    fn remote_diagnose_command_parses_json_flag() {
        let cli = Cli::try_parse_from(["nictui", "remote", "diagnose", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Remote {
                command: RemoteCommand::Diagnose(_)
            })
        ));
    }

    #[test]
    fn assess_remote_capability_marks_probe_as_not_evaluated() {
        let probe = ProbeResult {
            port: "/dev/cu.usbserial-210".to_string(),
            handshake_ok: true,
            endian: None,
            channel_endian: None,
            firmware_variant: Some(FirmwareVariant::NicSure),
        };

        let assessment = assess_remote_capability(&probe.port, &probe, None);
        assert_eq!(assessment.status, "not-evaluated");
        assert!(
            assessment
                .detail
                .contains("Probe does not exercise remote control")
        );
    }

    #[test]
    fn assess_remote_capability_marks_remote_collision_as_not_confirmed() {
        let probe = ProbeResult {
            port: "/dev/cu.usbserial-210".to_string(),
            handshake_ok: true,
            endian: None,
            channel_endian: None,
            firmware_variant: Some(FirmwareVariant::NicSure),
        };

        let assessment = assess_remote_capability(&probe.port, &probe, Some("remote-collision"));
        assert_eq!(assessment.status, "not-confirmed");
        assert!(assessment.detail.contains("remote_control_confirmed: true"));
        assert!(assessment.detail.contains("telemetry-primed"));
    }

    #[test]
    fn probe_view_includes_remote_capability_guidance() {
        let probe = ProbeResult {
            port: "/dev/cu.usbserial-210".to_string(),
            handshake_ok: true,
            endian: None,
            channel_endian: None,
            firmware_variant: Some(FirmwareVariant::NicSure),
        };

        let view = probe_view(&probe);
        assert_eq!(view.remote_capability, "not-evaluated");
        assert!(
            view.remote_hint
                .as_deref()
                .is_some_and(|hint| hint.contains("doctor"))
        );
    }

    #[test]
    fn summarize_remote_key_send_report_requires_decoded_delta_for_confirmation() {
        let report = RemoteKeySendReport {
            control: RemoteControlReport {
                label: "menu".to_string(),
                strategy: RemoteControlStrategy::RawKey,
                bytes_hex: "0B 00".to_string(),
                success: true,
                evidence: RemoteEvidenceKind::NoControlEvidence,
                reaction: Some(RemoteCommandReaction {
                    window_ms: 250,
                    rx_first_ms: Some(11),
                    surfaced_packets: 1,
                    unknown_packets: 0,
                    deltas: 0,
                }),
                detail: String::new(),
            },
        };

        let summary = summarize_remote_key_send_report(&report);
        assert!(summary.contains("Remote control is not yet confirmed"));
        assert!(summary.contains("rx-first=11ms"));
    }

    #[test]
    fn summarize_remote_key_send_report_confirms_decoded_delta() {
        let report = RemoteKeySendReport {
            control: RemoteControlReport {
                label: "menu".to_string(),
                strategy: RemoteControlStrategy::RawKey,
                bytes_hex: "0B 00".to_string(),
                success: true,
                evidence: RemoteEvidenceKind::ControlConfirmed,
                reaction: Some(RemoteCommandReaction {
                    window_ms: 250,
                    rx_first_ms: Some(9),
                    surfaced_packets: 1,
                    unknown_packets: 0,
                    deltas: 1,
                }),
                detail: String::new(),
            },
        };

        let summary = summarize_remote_key_send_report(&report);
        assert!(summary.contains("remote control is confirmed"));
        assert!(summary.contains("1 decoded state delta"));
    }

    #[test]
    fn summarize_remote_key_send_report_calls_out_no_reaction() {
        let report = RemoteKeySendReport {
            control: RemoteControlReport {
                label: "menu".to_string(),
                strategy: RemoteControlStrategy::RawKey,
                bytes_hex: "0B 00".to_string(),
                success: true,
                evidence: RemoteEvidenceKind::NoTelemetry,
                reaction: Some(RemoteCommandReaction {
                    window_ms: 250,
                    rx_first_ms: None,
                    surfaced_packets: 0,
                    unknown_packets: 0,
                    deltas: 0,
                }),
                detail: String::new(),
            },
        };

        let summary = summarize_remote_key_send_report(&report);
        assert!(summary.contains("no RX, packets, or decoded state delta"));
        assert!(summary.contains("250ms"));
    }

    #[test]
    fn summarizes_pvojh_collision_results() {
        let results = vec![
            PvojhSweepResult {
                stage: PvojhSweepStage::StartId,
                gap_ms: 0,
                start_rx: vec![0x4A],
                next_rx: Vec::new(),
                cleanup_rx: vec![0x45],
                verdict: "remote-collision-swallow".to_string(),
            },
            PvojhSweepResult {
                stage: PvojhSweepStage::StartId,
                gap_ms: 50,
                start_rx: vec![0x4A],
                next_rx: vec![0x02],
                cleanup_rx: vec![0x45],
                verdict: "remote-collision-echo".to_string(),
            },
        ];

        let (status, detail) = summarize_pvojh_results(&results);
        assert_eq!(status, "remote-collision");
        assert!(detail.contains("collides with remote-mode parsing"));
    }

    #[test]
    fn classify_remote_diagnose_detects_primed_telemetry_only() {
        let cases = vec![
            RemoteDiagnoseCaseView {
                label: "idle".to_string(),
                disable_radio: false,
                packet_count: 0,
                delta_count: 0,
                telemetry_observed: false,
                control_delta_observed: false,
                verdict: "silent".to_string(),
                summary: "No telemetry or command response was observed.".to_string(),
                control: None,
                failure: None,
            },
            RemoteDiagnoseCaseView {
                label: "telemetry-prime".to_string(),
                disable_radio: false,
                packet_count: 2,
                delta_count: 0,
                telemetry_observed: true,
                control_delta_observed: false,
                verdict: "telemetry-primed".to_string(),
                summary: "Prime burst woke telemetry.".to_string(),
                control: Some(RemoteDiagnoseControlView {
                    label: "telemetry-prime".to_string(),
                    strategy: "sequence".to_string(),
                    bytes_hex: "64 00 67".to_string(),
                    success: true,
                    detail: String::new(),
                    window_ms: 250,
                    rx_first_ms: Some(12),
                    surfaced_packets: 2,
                    unknown_packets: 0,
                    deltas: 0,
                }),
                failure: None,
            },
            RemoteDiagnoseCaseView {
                label: "telemetry-prime+hold-menu".to_string(),
                disable_radio: false,
                packet_count: 2,
                delta_count: 0,
                telemetry_observed: true,
                control_delta_observed: false,
                verdict: "primed-telemetry-carrythrough".to_string(),
                summary: "Telemetry stayed awake after priming. The follow-up control coincided with 1 surfaced packet(s) and 0 unknown packet(s), but no decoded control delta appeared. Treat this as carrythrough telemetry, not confirmed remote control.".to_string(),
                control: Some(RemoteDiagnoseControlView {
                    label: "hold-menu".to_string(),
                    strategy: "sequence".to_string(),
                    bytes_hex: "0B 00".to_string(),
                    success: true,
                    detail: String::new(),
                    window_ms: 250,
                    rx_first_ms: None,
                    surfaced_packets: 1,
                    unknown_packets: 0,
                    deltas: 0,
                }),
                failure: None,
            },
        ];

        assert_eq!(
            classify_remote_diagnose(&cases),
            "primed-telemetry-carrythrough"
        );
        assert!(summarize_remote_diagnose(&cases).contains("carrythrough telemetry"));
        assert!(summarize_remote_diagnose(&cases).contains("not confirmed remote control"));
    }

    #[test]
    fn classify_remote_diagnose_requires_decoded_delta_for_confirmed_control() {
        let cases = vec![RemoteDiagnoseCaseView {
            label: "menu".to_string(),
            disable_radio: false,
            packet_count: 1,
            delta_count: 0,
            telemetry_observed: true,
            control_delta_observed: false,
            verdict: "telemetry-after-command-no-control-delta".to_string(),
            summary: "This command surfaced telemetry or unknown packets, but none produced a decoded control delta. Treat it as session activity, not confirmed control.".to_string(),
            control: Some(RemoteDiagnoseControlView {
                label: "menu".to_string(),
                strategy: "sequence".to_string(),
                bytes_hex: "0B".to_string(),
                success: true,
                detail: String::new(),
                window_ms: 250,
                rx_first_ms: Some(10),
                surfaced_packets: 1,
                unknown_packets: 0,
                deltas: 0,
            }),
                failure: None,
        }];

        assert_eq!(
            classify_remote_diagnose(&cases),
            "rx-without-confirmed-control"
        );
        assert!(summarize_remote_diagnose(&cases).contains("Remote control is not yet confirmed"));
    }

    #[test]
    fn classify_remote_diagnose_marks_decoded_delta_as_confirmed_control() {
        let cases = vec![RemoteDiagnoseCaseView {
            label: "menu".to_string(),
            disable_radio: false,
            packet_count: 1,
            delta_count: 1,
            telemetry_observed: true,
            control_delta_observed: true,
            verdict: "confirmed-control-delta".to_string(),
            summary:
                "This command produced at least one decoded state delta, so control activity is confirmed."
                    .to_string(),
            control: Some(RemoteDiagnoseControlView {
                label: "menu".to_string(),
                strategy: "sequence".to_string(),
                bytes_hex: "0B".to_string(),
                success: true,
                detail: String::new(),
                window_ms: 250,
                rx_first_ms: Some(10),
                surfaced_packets: 1,
                unknown_packets: 0,
                deltas: 1,
            }),
                failure: None,
        }];

        assert_eq!(classify_remote_diagnose(&cases), "confirmed-control-delta");
        assert!(summarize_remote_diagnose(&cases).contains("remote control is confirmed"));
    }

    #[test]
    fn decodes_live_mode_remote_flag_block() {
        let mut block = vec![0u8; crate::protocol::BLOCK_SIZE];
        block[7] = 0b0101_1011;
        block[15] = 0b1000_0001;
        let decoded = decode_live_mode_block(0x0CA0, &block);

        assert!(decoded.iter().any(|line| line.contains("remote-kill=true")));
        assert!(decoded.iter().any(|line| line.contains("remote-halo=true")));
        assert!(
            decoded
                .iter()
                .any(|line| line.contains("alarm-mode=remote"))
        );
    }

    #[test]
    fn decodes_live_mode_key_assignments_from_reachable_block() {
        let mut block = vec![0u8; crate::protocol::BLOCK_SIZE];
        block[16] = 1;
        block[17] = 2;
        block[18] = 5;
        block[19] = 8;
        block[20] = 0;
        block[21] = 6;

        let decoded = decode_live_mode_block(0x0C80, &block);
        assert!(decoded.iter().any(|line| {
            line.contains("0x0C90..0x0C92 short-keys: top=radio side1=torch side2=alarm")
        }));
        assert!(decoded.iter().any(|line| {
            line.contains("0x0C93..0x0C95 long-keys: top=band side1=none side2=radio-alt")
        }));
    }

    #[test]
    fn rejects_live_mode_block_spans_that_overrun_eeprom() {
        let error = validate_live_block_span(0x1FE0, 2).unwrap_err().to_string();
        assert!(error.contains("exceeds"));
        assert!(validate_live_block_span(0x1FE0, 1).is_ok());
    }

    #[test]
    fn rejects_zero_live_mode_blocks() {
        let error = validate_live_block_span(0x0CA0, 0).unwrap_err().to_string();
        assert!(error.contains("at least 1"));
    }
}

fn finalize_doctor_report(
    report: &DoctorReport,
    output_dir: Option<&Path>,
    json: bool,
) -> Result<()> {
    if let Some(dir) = output_dir {
        write_json_output(report, Some(dir.join("doctor.json").as_path()))?;
    }

    if json {
        write_json_output(report, None)
    } else {
        print_doctor_report(report, output_dir);
        Ok(())
    }
}

fn print_doctor_report(report: &DoctorReport, output_dir: Option<&Path>) {
    println!("Doctor summary for {}", report.port);
    println!(
        "Handshake: {}",
        if report.handshake_ok { "ok" } else { "failed" }
    );
    if let Some(endian) = &report.endian {
        println!("Endian: {}", endian);
    }
    if let Some(endian) = &report.channel_endian {
        println!("Channel endian: {}", endian);
    }
    if let Some(firmware) = &report.firmware {
        println!("Firmware: {}", firmware);
    }
    println!("Live mode: {}", report.live_mode);
    println!("Remote control evidence: {}", report.remote_capability);

    for section in &report.sections {
        if section.ok {
            println!("- {}: ok ({})", section.name, section.detail);
            if let Some(output) = &section.output {
                println!("  output: {}", output);
            }
        } else {
            println!("- {}: failed ({})", section.name, section.detail);
        }
    }

    if let Some(dir) = output_dir {
        println!("Artifacts: {}", dir.display());
    }
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    port: String,
    handshake_ok: bool,
    endian: Option<String>,
    channel_endian: Option<String>,
    firmware: Option<String>,
    live_mode: String,
    remote_capability: String,
    sections: Vec<DoctorSection>,
}

#[derive(Debug, Serialize)]
struct DoctorSection {
    name: String,
    ok: bool,
    detail: String,
    output: Option<String>,
}

impl DoctorSection {
    fn success(name: &str, detail: String, output: Option<PathBuf>) -> Self {
        Self {
            name: name.to_string(),
            ok: true,
            detail,
            output: output.map(|path| path.display().to_string()),
        }
    }

    fn failure(name: &str, detail: String) -> Self {
        Self {
            name: name.to_string(),
            ok: false,
            detail,
            output: None,
        }
    }
}

#[derive(Default)]
struct ProgressPrinter {
    last_bucket: Option<i32>,
}

impl ProgressPrinter {
    fn handle(&mut self, event: ProgressEvent) {
        match event {
            ProgressEvent::Status(message) => {
                self.last_bucket = None;
                eprintln!("{message}");
            }
            ProgressEvent::Progress(value) => {
                let percent = (value * 100.0).round() as i32;
                let bucket = (percent / 5) * 5;
                if self.last_bucket != Some(bucket) {
                    self.last_bucket = Some(bucket);
                    eprintln!("Progress: {bucket}%");
                }
            }
        }
    }
}
