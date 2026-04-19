use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::channel_file::{
    ChannelFileFormat, infer_channel_file_format, load_channels_from_path, save_channels_to_writer,
};
use crate::device::{
    FirmwareVariant, PortCandidate, PortKind, ProgressEvent, RemoteMonitorEvent,
    RemoteMonitorOptions, flash_firmware, inspect_codeplug, list_port_candidates, list_ports,
    monitor_remote, probe_port, read_band_plans, read_channels, read_codeplug, read_dtmf_presets,
    read_scan_presets, read_settings, resolve_port, send_remote_key, update_dtmf_preset,
    update_scan_preset, validate_band_plans_payload, validate_channels,
    validate_dtmf_presets_payload, validate_scan_presets, write_band_plans, write_channels,
    write_codeplug, write_dtmf_presets, write_scan_presets, write_settings,
};
use crate::protocol::codeplug::{load_codeplug, save_codeplug};
use crate::protocol::{
    BandPlan, Channel, DTMFPreset, Endianness, RadioProtocol, SETTINGS_METADATA, ScanPreset,
    SettingType, SettingsBlock,
};
use crate::skill::{
    SkillInstallTarget, SupportedAgent, bundled_skill_dir, bundled_skill_markdown, detected_agents,
    install_bundled_skill,
};

const CLI_AFTER_HELP: &str = "\
Recommended workflow:
  1. nictui ports --verbose
  2. nictui probe
  3. nictui doctor --output-dir ./.live-debug/session --json
  4. nictui <section> get/read ...
  5. nictui <section> set/update/write --validate-only
  6. nictui <section> set/update/write
  7. nictui <section> get/read ...";

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
    /// Probe the radio and report handshake, firmware, and endianness
    #[command(visible_alias = "detect")]
    Probe(ProbeArgs),
    /// Run a safe read-only radio health check and optionally save artifacts
    #[command(visible_alias = "check")]
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
    /// Send remote-control keys or inspect remote packets
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
    #[arg(long)]
    pub port: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct PortArgs {
    /// Serial port to use. If omitted, NicTUI auto-selects the only detected radio-like port.
    #[arg(long)]
    pub port: Option<String>,
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
    /// Directory to write read-only artifacts and the final doctor report
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
    /// Send a single remote-control key press to the radio
    #[command(visible_alias = "send")]
    Key(RemoteKeyArgs),
    /// Keep a remote session open and print decoded packets
    #[command(visible_alias = "watch")]
    Monitor(RemoteMonitorArgs),
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
    #[arg(long, default_value_t = true)]
    pub reboot: bool,
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
    /// Send one or more keys during monitoring to exercise the session
    #[arg(long = "send", value_enum)]
    pub send: Vec<RemoteKey>,
    /// Delay between scripted key presses in milliseconds
    #[arg(long, default_value_t = 350)]
    pub send_interval_ms: u64,
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

pub enum Dispatch {
    LaunchTui { port: Option<String> },
    Exit,
}

pub fn dispatch(cli: Cli) -> Result<Dispatch> {
    match cli.command {
        None => Ok(Dispatch::LaunchTui { port: None }),
        Some(Commands::Tui(args)) => Ok(Dispatch::LaunchTui { port: args.port }),
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
                RemoteCommand::Monitor(args) => run_remote_monitor(args)?,
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
    hint: Option<String>,
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
        if matches!(firmware, FirmwareVariant::Stock) {
            println!("Hint: install NicSure mod firmware before using NicTUI read/write commands.");
        }
    }

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
        hint: if matches!(result.firmware_variant, Some(FirmwareVariant::Stock)) {
            Some(
                "Install NicSure mod firmware before using NicTUI live read/write commands."
                    .to_string(),
            )
        } else {
            None
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
        details.push(format!("maker={manufacturer}"));
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
    write_settings(&port, &settings, endian, true, |event| {
        progress.handle(event)
    })
}

fn run_get_setting(args: ReadSettingArgs) -> Result<()> {
    let setting_index = resolve_setting_selector(&args.setting)?;
    let port = resolve_port_for_args(&args.port)?;
    let mut progress = ProgressPrinter::default();
    let (settings, _) = read_settings(&port, |event| progress.handle(event))?;
    let view = build_setting_view(&settings, setting_index);
    write_json_output(&view, args.output.as_deref())
}

fn run_set_setting(args: SetSettingArgs) -> Result<()> {
    let setting_index = resolve_setting_selector(&args.setting)?;
    let setting_value = parse_setting_value(setting_index, &args.value)?;

    if args.validate_only {
        print_setting_change_validation_summary(setting_index, setting_value);
        return Ok(());
    }

    let port = resolve_port_for_args(&args.port)?;
    let mut read_progress = ProgressPrinter::default();
    let (mut settings, endian) = read_settings(&port, |event| read_progress.handle(event))?;
    settings.set_value(setting_index, setting_value);
    validate_settings_payload(&settings)?;

    let mut write_progress = ProgressPrinter::default();
    write_settings(&port, &settings, endian, !args.no_reboot, |event| {
        write_progress.handle(event)
    })?;
    eprintln!(
        "Updated setting {} on {}",
        SETTINGS_METADATA[setting_index].menu_num, port
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
    write_codeplug(&port, &codeplug, args.reboot, |event| {
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
    send_remote_key(&port, args.key.as_key_code())?;
    eprintln!("Sent {:?} to {}", args.key, port);
    Ok(())
}

fn run_remote_monitor(args: RemoteMonitorArgs) -> Result<()> {
    let port = resolve_port_for_args(&args.port)?;
    let options = RemoteMonitorOptions {
        duration: std::time::Duration::from_secs(args.duration),
        include_raw_logs: args.raw,
        scripted_keys: args
            .send
            .iter()
            .copied()
            .map(RemoteKey::as_key_code)
            .collect(),
        key_interval: std::time::Duration::from_millis(args.send_interval_ms),
    };

    eprintln!(
        "Monitoring remote packets on {} for {}s",
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
            RemoteMonitorEvent::Packet(packet) => {
                println!("[{timestamp}] REMOTE {}", packet.summary());
            }
        }
    })?;

    eprintln!("Captured {} remote packets from {}", packet_count, port);
    Ok(())
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
    let port = resolve_port_for_args(&args.port)?;
    let probe = probe_port(&port)?;

    let mut report = DoctorReport {
        port: port.clone(),
        handshake_ok: probe.handshake_ok,
        endian: probe.endian.map(format_endianness),
        channel_endian: probe.channel_endian.map(format_endianness),
        firmware: probe.firmware_variant.map(format_firmware_variant),
        sections: Vec::new(),
    };

    let output_dir = if let Some(dir) = args.output_dir {
        fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
        Some(dir)
    } else {
        None
    };

    if !probe.handshake_ok {
        report.sections.push(DoctorSection::failure(
            "probe",
            "Handshake failed".to_string(),
        ));
        finalize_doctor_report(&report, output_dir.as_deref(), args.json)?;
        bail!("Handshake failed for {}", port);
    }

    if matches!(probe.firmware_variant, Some(FirmwareVariant::Stock)) {
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
            RemoteKey::Digit0 => 0x80,
            RemoteKey::Digit1 => 0x81,
            RemoteKey::Digit2 => 0x82,
            RemoteKey::Digit3 => 0x83,
            RemoteKey::Digit4 => 0x84,
            RemoteKey::Digit5 => 0x85,
            RemoteKey::Digit6 => 0x86,
            RemoteKey::Digit7 => 0x87,
            RemoteKey::Digit8 => 0x88,
            RemoteKey::Digit9 => 0x89,
            RemoteKey::Menu => 0x8A,
            RemoteKey::Up => 0x8B,
            RemoteKey::Down => 0x8C,
            RemoteKey::Exit => 0x8D,
            RemoteKey::Star => 0x8E,
            RemoteKey::Pound => 0x8F,
            RemoteKey::PttA => 0x90,
            RemoteKey::PttB => 0x91,
            RemoteKey::Flashlight => 0x92,
            RemoteKey::Vm => 0x94,
        }
    }
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
    if !probe.handshake_ok {
        bail!("Handshake failed for {}", port);
    }
    if matches!(probe.firmware_variant, Some(FirmwareVariant::Stock)) {
        bail!(
            "{} appears to be running stock/original firmware. Install NicSure mod firmware before using NicTUI live read/write features.",
            port
        );
    }
    Ok(probe.endian.unwrap_or(crate::protocol::Endianness::Big))
}

fn resolve_port_for_args(args: &PortArgs) -> Result<String> {
    let port = resolve_port(args.port.as_deref())?;
    if args.port.is_none() && list_ports()?.len() > 1 {
        eprintln!("Auto-detected radio port: {}", port);
    }
    Ok(port)
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
    use super::{ChannelsCommand, Cli, Commands, SkillCommand, validate_channel_range};
    use clap::{CommandFactory, Parser};

    #[test]
    fn validate_channel_range_accepts_inclusive_range() {
        assert_eq!(validate_channel_range(8, 10).unwrap(), vec![8, 9, 10]);
    }

    #[test]
    fn validate_channel_range_rejects_reversed_range() {
        assert!(validate_channel_range(10, 8).is_err());
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
