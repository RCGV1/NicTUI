use nictui::protocol::{Endianness, RadioProtocol};
use std::thread;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let ports = serialport::available_ports()?;
    if ports.is_empty() {
        println!("No serial ports found");
        return Ok(());
    }

    println!("Available ports:");
    for p in &ports {
        println!("  {}", p.port_name);
    }

    let port_name = ports
        .iter()
        .find(|p| {
            p.port_name.to_lowercase().contains("usb") || p.port_name.to_lowercase().contains("wch")
        })
        .map(|p| &p.port_name)
        .unwrap_or(&ports[0].port_name);

    println!("Connecting to {}...", port_name);

    let mut proto = RadioProtocol::new(port_name)?;

    println!("Handshaking...");
    let mut connected = false;
    for i in 0..5 {
        match proto.handshake() {
            Ok(true) => {
                connected = true;
                break;
            }
            _ => {
                println!("Handshake attempt {} failed, retrying...", i + 1);
                thread::sleep(Duration::from_millis(500));
            }
        }
    }

    if !connected {
        println!("Handshake failed after 5 attempts");
        return Ok(());
    }

    println!("Detecting Endianness...");
    let blk_240 = proto.read_block(240)?;
    let magic_vhf = blk_240[0];
    let endian = if magic_vhf == 0x57 {
        println!("Detected Little Endian (Magic: 0x{:02X})", magic_vhf);
        Endianness::Little
    } else {
        println!("Detected Big Endian (Magic: 0x{:02X})", magic_vhf);
        Endianness::Big
    };

    println!("Reading Settings...");
    let start_blk = 0x1900 / 32;
    let mut data = Vec::new();
    for i in 0..4 {
        let blk = proto.read_block((start_blk + i) as u8)?;
        println!("Block {}: {:02X?}", i, blk);
        data.extend_from_slice(&blk);
    }

    let settings = RadioProtocol::parse_settings_block(&data, endian);
    println!("Parsed Settings:");
    println!("Magic: 0x{:04X}", settings.magic);
    println!("Squelch: {}", settings.squelch);
    println!("Step: {}", settings.step);

    if settings.magic != 0xD82F {
        println!(
            "WARNING: Magic number mismatch! Expected 0xD82F, got 0x{:04X}",
            settings.magic
        );
    }

    // Modify Squelch
    let original_squelch = settings.squelch;
    let new_squelch = if original_squelch == 1 { 2 } else { 1 };
    println!(
        "Modifying Squelch from {} to {}...",
        original_squelch, new_squelch
    );

    let mut new_settings = settings.clone();
    new_settings.squelch = new_squelch;

    let packed_data = RadioProtocol::pack_settings_block(&new_settings, endian);

    println!("Writing Settings...");
    for i in 0..4 {
        let blk_data = &packed_data[i * 32..(i + 1) * 32];
        println!("Writing Block {}: {:02X?}", i, blk_data);
        if !proto.write_block((start_blk + i) as u8, blk_data)? {
            println!("Failed to write block {}", i);
            return Ok(());
        }
    }

    println!("Rebooting...");
    proto.reboot()?;

    // Wait for reboot
    thread::sleep(Duration::from_secs(12));

    // Re-connect and verify
    println!("Re-connecting to verify...");
    let mut proto = RadioProtocol::new(port_name)?;
    if !proto.handshake()? {
        println!("Handshake failed after reboot");
        return Ok(());
    }

    println!("Reading Settings again...");
    let mut data = Vec::new();
    for i in 0..4 {
        let blk = proto.read_block((start_blk + i) as u8)?;
        data.extend_from_slice(&blk);
    }
    let verify_settings = RadioProtocol::parse_settings_block(&data, endian);
    println!("Verified Squelch: {}", verify_settings.squelch);

    if verify_settings.squelch == new_squelch {
        println!("SUCCESS: Settings updated correctly!");

        // Restore original
        println!("Restoring original squelch...");
        let mut restore_settings = verify_settings.clone();
        restore_settings.squelch = original_squelch;
        let packed_restore = RadioProtocol::pack_settings_block(&restore_settings, endian);
        for i in 0..4 {
            let blk_data = &packed_restore[i * 32..(i + 1) * 32];
            proto.write_block((start_blk + i) as u8, blk_data)?;
        }
        proto.reboot()?;
    } else {
        println!("FAILURE: Settings did not update.");
    }

    Ok(())
}
