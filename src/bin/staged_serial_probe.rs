use anyhow::{Result, anyhow, bail};
use clap::Parser;
use nictui::protocol::RadioProtocol;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "staged_serial_probe",
    about = "Send staged byte sequences to one radio session and log each response window"
)]
struct Args {
    #[arg(long)]
    port: String,
    /// Steps in the form "bytes@rx_ms;bytes@rx_ms", for example "4B@200;50 56 4F 4A 48 5C 14@400;02@800"
    #[arg(long)]
    steps: String,
    /// Delay between stages in milliseconds
    #[arg(long, default_value_t = 50)]
    gap_ms: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let steps = parse_steps(&args.steps)?;
    let mut proto = RadioProtocol::new(&args.port)?;

    for (index, step) in steps.iter().enumerate() {
        if index > 0 {
            std::thread::sleep(Duration::from_millis(args.gap_ms));
        }

        println!("step{} tx {}", index + 1, format_hex(&step.bytes));
        proto.send_bytes(&step.bytes)?;

        let deadline = Instant::now() + Duration::from_millis(step.rx_ms);
        let mut received = Vec::new();
        while Instant::now() < deadline {
            if let Some(byte) = proto.read_byte()? {
                received.push(byte);
                println!("step{} rx {:02X}", index + 1, byte);
            }
        }

        if received.is_empty() {
            println!("step{} rx <none>", index + 1);
        } else {
            println!("step{} rx-line {}", index + 1, format_hex(&received));
        }
    }

    Ok(())
}

#[derive(Debug)]
struct ProbeStep {
    bytes: Vec<u8>,
    rx_ms: u64,
}

fn parse_steps(value: &str) -> Result<Vec<ProbeStep>> {
    let mut steps = Vec::new();
    for raw_step in value
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let (bytes_part, rx_part) = raw_step
            .rsplit_once('@')
            .ok_or_else(|| anyhow!("Invalid step '{raw_step}', expected bytes@rx_ms"))?;
        let bytes = parse_bytes(bytes_part)?;
        let rx_ms = rx_part
            .trim()
            .parse::<u64>()
            .map_err(|error| anyhow!("Invalid rx_ms '{rx_part}' in step '{raw_step}': {error}"))?;
        steps.push(ProbeStep { bytes, rx_ms });
    }

    if steps.is_empty() {
        bail!("Provide at least one stage with --steps");
    }

    Ok(steps)
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
        bail!("Each stage must include at least one byte");
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
