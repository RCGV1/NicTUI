use nictui::protocol::{Endianness, RadioProtocol};

fn main() -> anyhow::Result<()> {
    let ports = serialport::available_ports()?;
    if ports.is_empty() {
        println!("No serial ports found");
        return Ok(());
    }
    let port_name = &ports[0].port_name;
    println!("Using port: {}", port_name);

    let mut proto = RadioProtocol::new(port_name)?;
    println!("Handshaking...");
    if !proto.handshake()? {
        println!("Handshake failed");
        return Ok(());
    }

    // 1. Detect Endianness
    println!("Detecting endianness...");
    let blk240 = proto.read_block(240)?;
    let endian = if blk240[0] == 0x57 {
        Endianness::Little
    } else {
        Endianness::Big
    };
    println!("Detected endianness: {:?}", endian);

    // 2. Read Channel 1 (Block 2)
    println!("Reading Channel 1 (Block 2)...");
    let raw_ch1 = proto.read_block(2)?;
    let ch1 = RadioProtocol::parse_channel(&raw_ch1, 1, endian)
        .expect("Channel 1 should be programmed for this test");
    println!(
        "Original Channel 1: Name='{}', RX={}",
        ch1.name, ch1.rx_freq
    );

    // 3. Modify Channel 1
    let mut modified_ch1 = ch1.clone();
    modified_ch1.name = format!("TEST_{}", chrono::Local::now().format("%H%M%S"));
    println!("Writing modified Channel 1: Name='{}'", modified_ch1.name);
    let packed = RadioProtocol::pack_channel(&modified_ch1, endian);
    proto.write_block(2, &packed)?;

    // 4. Read back and verify
    println!("Reading back Channel 1...");
    let raw_ch1_new = proto.read_block(2)?;
    let ch1_new = RadioProtocol::parse_channel(&raw_ch1_new, 1, endian)
        .expect("Channel 1 should still be programmed");
    println!("New Channel 1: Name='{}'", ch1_new.name);

    if ch1_new.name == modified_ch1.name {
        println!("SUCCESS: Channel 1 updated correctly at Block 2");
    } else {
        println!(
            "FAILURE: Channel 1 name mismatch. Expected '{}', got '{}'",
            modified_ch1.name, ch1_new.name
        );
    }

    // 5. Restore original
    println!("Restoring original Channel 1...");
    let packed_orig = RadioProtocol::pack_channel(&ch1, endian);
    proto.write_block(2, &packed_orig)?;
    println!("Done.");

    Ok(())
}
