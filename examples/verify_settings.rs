use nictui::protocol::{Endianness, RadioProtocol};

fn main() -> anyhow::Result<()> {
    let ports = serialport::available_ports()?;
    if ports.is_empty() {
        println!("No serial ports found!");
        return Ok(());
    }

    let port_name = &ports[0].port_name;
    println!("Using port: {}", port_name);

    let mut proto = RadioProtocol::new(port_name)?;

    println!("Handshaking...");
    if !proto.handshake()? {
        println!("Handshake failed!");
        return Ok(());
    }

    println!("Reading current settings...");
    let start_blk = 0x1900 / 32;
    let mut data = Vec::new();
    for i in 0..4 {
        let blk = proto.read_block((start_blk + i) as u8)?;
        data.extend_from_slice(&blk);
    }

    let mut settings = RadioProtocol::parse_settings_block(&data, Endianness::Little);
    let original_brightness = settings.lcd_brightness;
    println!("Current LCD Brightness: {}", original_brightness);

    let test_brightness = if original_brightness == 10 { 20 } else { 10 };
    println!("Setting LCD Brightness to: {}", test_brightness);
    settings.lcd_brightness = test_brightness;

    println!("Writing settings...");
    let packed = RadioProtocol::pack_settings_block(&settings, Endianness::Little);
    for i in 0..4 {
        let blk_data = &packed[i * 32..(i + 1) * 32];
        if !proto.write_block((start_blk + i) as u8, blk_data)? {
            println!("Failed to write block {}", i);
            return Ok(());
        }
    }

    println!("Verifying...");
    let mut verify_data = Vec::new();
    for i in 0..4 {
        let blk = proto.read_block((start_blk + i) as u8)?;
        verify_data.extend_from_slice(&blk);
    }
    let verified_settings = RadioProtocol::parse_settings_block(&verify_data, Endianness::Little);
    println!(
        "Verified LCD Brightness: {}",
        verified_settings.lcd_brightness
    );

    if verified_settings.lcd_brightness == test_brightness {
        println!("SUCCESS: Settings write verified!");
    } else {
        println!("FAILURE: Settings write verification failed!");
    }

    println!("Restoring original brightness...");
    settings.lcd_brightness = original_brightness;
    let packed = RadioProtocol::pack_settings_block(&settings, Endianness::Little);
    for i in 0..4 {
        let blk_data = &packed[i * 32..(i + 1) * 32];
        proto.write_block((start_blk + i) as u8, blk_data)?;
    }
    println!("Done.");

    Ok(())
}
