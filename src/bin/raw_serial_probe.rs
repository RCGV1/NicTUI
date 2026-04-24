use anyhow::{Result, anyhow, bail};
use clap::Parser;
use nictui::protocol::RadioProtocol;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "raw_serial_probe",
    about = "Send raw bytes to a radio port and print every byte received back"
)]
struct Args {
    #[arg(long)]
    port: String,
    #[arg(long)]
    bytes: String,
    #[arg(long, default_value_t = 1)]
    repeat: u32,
    #[arg(long, default_value_t = 150)]
    gap_ms: u64,
    #[arg(long, default_value_t = 800)]
    rx_ms: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let bytes = parse_bytes(&args.bytes)?;
    let mut proto = RadioProtocol::new(&args.port)?;

    for iteration in 0..args.repeat {
        if iteration > 0 {
            std::thread::sleep(Duration::from_millis(args.gap_ms));
        }

        println!("tx {}", format_hex(&bytes));
        proto.send_bytes(&bytes)?;

        let deadline = Instant::now() + Duration::from_millis(args.rx_ms);
        let mut received = Vec::new();
        while Instant::now() < deadline {
            if let Some(byte) = proto.read_byte()? {
                received.push(byte);
                println!("rx {:02X}", byte);
            }
        }

        if received.is_empty() {
            println!("rx <none>");
        } else {
            println!("rx-line {}", format_hex(&received));
        }
    }

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
        bail!("Provide at least one byte with --bytes");
    }

    Ok(bytes)
}

fn format_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
