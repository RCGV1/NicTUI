use anyhow::{Result, anyhow, bail};
use clap::Parser;
use nictui::device::{RemoteMonitorEvent, RemoteMonitorOptions, monitor_remote};
use nictui::remote::RemoteControlCommand;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "remote_probe",
    about = "Low-level remote-control probe for TD-H3 radios"
)]
struct Args {
    #[arg(long)]
    port: String,
    #[arg(long)]
    bytes: String,
    #[arg(long, default_value_t = 1)]
    repeat: u32,
    #[arg(long, default_value_t = 80)]
    gap_ms: u64,
    #[arg(long, default_value_t = 250)]
    repeat_gap_ms: u64,
    #[arg(long, default_value_t = 0)]
    hold_ms: u64,
    #[arg(long, default_value_t = 250)]
    pre_ms: u64,
    #[arg(long, default_value_t = 2000)]
    post_ms: u64,
    #[arg(long)]
    disable_radio: bool,
    #[arg(long)]
    raw: bool,
    #[arg(long, default_value_t = 0)]
    recover_retries: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let bytes = parse_bytes(&args.bytes)?;
    let command = RemoteControlCommand::sequence(
        "remote-probe",
        bytes,
        Duration::from_millis(args.gap_ms),
        args.repeat,
        Duration::from_millis(args.hold_ms),
    );
    let duration =
        Duration::from_millis(args.pre_ms + args.post_ms + 800) + command.estimated_duration();

    let packet_count = monitor_remote(
        &args.port,
        &RemoteMonitorOptions {
            duration,
            include_raw_logs: args.raw,
            suppress_idle_zero_logs: true,
            scripted_commands: vec![command],
            command_start_delay: Duration::from_millis(args.pre_ms),
            key_interval: Duration::from_millis(args.repeat_gap_ms),
            disable_radio_before_remote: args.disable_radio,
            recover_retries: args.recover_retries,
        },
        |event| match event {
            RemoteMonitorEvent::Status(message) => println!("status {message}"),
            RemoteMonitorEvent::Log(message) => println!("log {message}"),
            RemoteMonitorEvent::Phase(phase) => println!("phase {phase}"),
            RemoteMonitorEvent::Control(report) => println!(
                "control {} [{}] {}",
                report.label, report.strategy, report.detail
            ),
            RemoteMonitorEvent::Packet(packet) => println!("packet {}", packet.summary()),
            RemoteMonitorEvent::Delta(delta) => println!("delta {delta}"),
        },
    )?;

    println!("captured {packet_count} decoded packet(s)");
    Ok(())
}

fn parse_bytes(value: &str) -> Result<Vec<u8>> {
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
        bail!("Provide at least one byte with --bytes, for example --bytes 8A,FF");
    }

    Ok(bytes)
}
