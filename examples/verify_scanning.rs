use anyhow::Result;
use nictui::protocol::RadioProtocol;
use std::thread;
use std::time::Duration;

fn main() -> Result<()> {
    let ports = serialport::available_ports()?;
    if ports.is_empty() {
        println!("No serial ports found");
        return Ok(());
    }

    let port_name = &ports[0].port_name;
    println!("Connecting to {}...", port_name);

    let mut proto = RadioProtocol::new(port_name)?;

    println!("Handshaking...");
    if !proto.handshake()? {
        println!("Handshake failed");
        return Ok(());
    }

    // Read Scan Presets Block (Block 215 = 0x1AE0)
    // 0x1AE0 / 32 = 215
    let block_num = 215;
    println!("Reading Block {} (0x{:X})...", block_num, block_num * 32);

    let data = proto.read_block(block_num)?;
    println!("Data: {:02X?}", data);

    // Parse first preset
    // struct {
    //     ul32 startscanfreq;
    //     ul16 numbersearches;
    //     u8 squelchscan;
    //     u8 squelchtailscan;
    //     ul16 stepscan;
    //     u8 scanhold;
    //     u8 scantail;
    //     u8 updatescan;
    //     u8 modulationscan;
    // } scanpresets[10];

    // Size is 14 bytes per preset?
    // 4 + 2 + 1 + 1 + 2 + 1 + 1 + 1 + 1 = 14 bytes.

    // Let's print the first 14 bytes
    if data.len() >= 14 {
        let preset_bytes = &data[0..14];
        println!("Preset 1 Bytes: {:02X?}", preset_bytes);

        let start_freq = u32::from_le_bytes(preset_bytes[0..4].try_into()?);
        println!(
            "Start Freq: {} ({} MHz)",
            start_freq,
            start_freq as f32 / 100000.0
        );
    }

    Ok(())
}
