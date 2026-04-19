use anyhow::Result;
use nictui::protocol::Endianness;
use nictui::protocol::radio::RadioProtocol;
use std::time::Duration;

fn main() -> Result<()> {
    let ports = serialport::available_ports()?;
    if ports.is_empty() {
        println!("No serial ports found");
        return Ok(());
    }

    println!("Found {} ports:", ports.len());
    for p in &ports {
        println!("  - {}", p.port_name);
    }

    for port in ports {
        let port_name = &port.port_name;
        println!("\nTrying port: {}...", port_name);

        let mut proto = match RadioProtocol::new(port_name) {
            Ok(p) => p,
            Err(e) => {
                println!("  Failed to open port: {}", e);
                continue;
            }
        };

        println!("  Handshaking...");
        let mut connected = false;
        for _attempt in 0..3 {
            if proto.handshake().unwrap_or(false) {
                connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }

        if !connected {
            println!("  Handshake failed on {}", port_name);
            continue;
        }

        println!("  Connected to {}!", port_name);
        println!("  Reading Channel 0 (Block 1)...");

        // Channel 0 is at block 1 (0 + 1)
        match proto.read_block(1) {
            Ok(data) => {
                println!("  Raw Data: {:02X?}", data);

                // Parse channel
                if let Some(ch) = RadioProtocol::parse_channel(&data, 0, Endianness::Big) {
                    println!("  Parsed Channel 0:");
                    println!("    Name: {}", ch.name);
                    println!("    RX Freq: {}", ch.rx_freq);
                    println!("    TX Freq: {}", ch.tx_freq);
                    println!("    Power (Parsed): {}", ch.power);
                    println!("    Power (Raw Byte at offset 12): 0x{:02X}", data[12]);
                    return Ok(()); // Found it!
                } else {
                    println!("    Failed to parse channel 0");
                }
            }
            Err(e) => {
                println!("  Failed to read block 1: {}", e);
            }
        }
    }

    println!("\nCould not find radio on any port.");
    Ok(())
}
