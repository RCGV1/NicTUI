use serialport;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::time::{Duration, Instant};
use std::thread;

const BAUD_RATE: u32 = 115200;
const INIT_SEQUENCE: [u8; 36] = [
    0xA0, 0xEE, 0x74, 0x71, 0x07, 0x74,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
    0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55
];

fn format_hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02X} ", b)).collect()
}

fn log_tx(data: &[u8]) {
    println!("[TX] {} bytes: {}", data.len(), format_hex(data));
}

fn log_rx(data: &[u8]) {
    println!("[RX] {} bytes: {}", data.len(), format_hex(data));
}

fn main() -> anyhow::Result<()> {
    println!("\n=== Simple BIN Flasher (JavaScript Protocol) ===\n");
    
    let default_firmware = "/Users/benjaminfaershtein/Downloads/tdh3/nicfwH3_V2.52.22-logo.bin";
    
    println!("\nAvailable serial ports:");
    let ports = serialport::available_ports().unwrap_or_default();
    for p in &ports {
        println!("  - {} ({:?})", p.port_name, p.port_type);
    }
    
    let args: Vec<String> = env::args().collect();
    
    let firmware_path: String = if args.len() > 1 {
        args[1].clone()
    } else {
        default_firmware.to_string()
    };
    
    let port_name: String = if args.len() > 2 {
        args[2].clone()
    } else if !ports.is_empty() {
        ports[0].port_name.clone()
    } else {
        println!("No serial ports found!");
        return Ok(());
    };
    
    println!("\nLoading firmware: {}", firmware_path);
    let firmware_data = fs::read(&firmware_path)?;
    let rounded_len = ((firmware_data.len() + 31) / 32) * 32;
    let last_block = (rounded_len / 32) as u16;
    println!("Firmware size: {} bytes, {} blocks (rounded to {})", firmware_data.len(), last_block + 1, rounded_len);
    
    println!("\nUsing port: {}", port_name);
    println!("Usage: {} <firmware.bin> [port]", args[0]);
    
    let mut port = serialport::new(&port_name, BAUD_RATE)
        .timeout(Duration::from_millis(50))
        .open()?;
    
    println!("Port opened!");
    
    println!("\n=== INSTRUCTIONS ===");
    println!("1. Turn OFF your radio");
    println!("2. Hold PTT (H3) or Flashlight (H8) button");
    println!("3. While holding button, turn ON radio");
    println!("4. Press Enter to start (you have 15 seconds)");
    println!("\nMake sure radio is in flash mode (holding button) before pressing Enter!");
    
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    
    println!("\n=== Waiting for 0xA5 handshake ===");
    
    let start_time = Instant::now();
    let timeout_secs = 60;
    let mut a5_count = 0;
    let mut detected = false;
    let mut init_sent = false;
    let mut flashing = false;
    let mut block: u16 = 0;
    let mut consecutive_a5_after_init = 0;
    let mut need_to_send_block = true;
    let mut block_send_time = Instant::now();  // Track when last block was sent
    
    loop {
        if start_time.elapsed().as_secs() > timeout_secs {
            println!("TIMEOUT: No response from radio");
            return Ok(());
        }
        
        // Send block if needed (first block or after ACK)
        if flashing && need_to_send_block && block <= last_block {
            let is_last_block = block == last_block;
            
            if is_last_block {
                println!("\n>>> Sending FINAL block {}...", block);
            } else {
                println!("\n>>> Sending block {}...", block);
            }
            
            let mut packet = [0u8; 36];
            packet[0] = if is_last_block { 0xA2 } else { 0xA1 };
            packet[1] = ((block >> 8) & 0xFF) as u8;
            packet[2] = (block & 0xFF) as u8;
            
            let start_idx = (block as usize) * 32;
            if start_idx + 32 <= firmware_data.len() {
                packet[4..36].copy_from_slice(&firmware_data[start_idx..start_idx + 32]);
            } else {
                packet[4..4 + (firmware_data.len() - start_idx)].copy_from_slice(&firmware_data[start_idx..]);
            }
            
            let mut checksum: u8 = 0;
            for i in 4..36 {
                checksum = checksum.wrapping_add(packet[i]);
            }
            packet[3] = checksum;
            
            log_tx(&packet);
            port.write_all(&packet)?;
            port.flush()?;
            
            // Small delay to allow radio to process the block
            thread::sleep(Duration::from_millis(20));
            
            let progress = (block + 1) as f64 / (last_block as f64 + 1.0);
            println!("Progress: {:.1}% (block {} / {})", progress * 100.0, block, last_block);
            
            block += 1;
            need_to_send_block = false;
            block_send_time = Instant::now();  // Reset ACK timeout
        }
        
        // Check for completion - if we've sent all blocks, wait for final ACK then finish
        if flashing && !need_to_send_block && block > last_block {
            // We've sent all blocks, wait for final ACK or timeout
            if block_send_time.elapsed() > Duration::from_millis(500) {
                println!("\n=== FLASHING COMPLETE! ===");
                println!("Successfully flashed {} blocks", last_block + 1);
                println!("\nTurn OFF radio and back ON to boot new firmware.");
                return Ok(());
            }
        }
        
        // Check for ACK timeout - if no A3 within 500ms after sending block, resend
        if flashing && !need_to_send_block && block <= last_block && block_send_time.elapsed() > Duration::from_millis(500) {
            println!(">>> No ACK within 500ms, resending block {}...", block - 1);
            block -= 1;  // Resend the same block
            need_to_send_block = true;
        }
        
        let mut buf = [0u8; 64];
        match port.read(&mut buf) {
            Ok(n) if n > 0 => {
                log_rx(&buf[..n]);
                
                for i in 0..n {
                    let byte = buf[i];
                    
                    if flashing {
                        if byte == 0xA3 {
                            println!(">>> ACK received for block {}", block - 1);
                            need_to_send_block = true;
                        }
                    } else if byte == 0xA5 {
                        a5_count += 1;
                        
                        if !detected {
                            // First A5 detected - radio is in flash mode
                            detected = true;
                            println!("\n>>> 0xA5 DETECTED! Radio is in flash mode.");
                            println!(">>> Sending INIT_SEQUENCE...");
                            
                            thread::sleep(Duration::from_millis(50));
                            log_tx(&INIT_SEQUENCE);
                            port.write_all(&INIT_SEQUENCE)?;
                            port.flush()?;
                            init_sent = true;
                            
                            a5_count = 0;
                            consecutive_a5_after_init = 0;
                        } else if init_sent {
                            // After INIT, radio still sending A5 - it's ready!
                            consecutive_a5_after_init += 1;
                            if consecutive_a5_after_init >= 3 {
                                println!("\n>>> Radio still sending 0xA5 - READY!");
                                println!(">>> Starting flash...");
                                flashing = true;
                                consecutive_a5_after_init = 0;
                            }
                        }
                    } else if byte == 0xA3 {
                        if init_sent {
                            println!("\n>>> 0xA3 received - RADIO READY!");
                            println!(">>> Starting flash...");
                            flashing = true;
                        }
                    } else if byte != 0x00 {
                        if init_sent && !flashing {
                            println!("\n>>> Non-A5/A3 byte after INIT - RADIO READY!");
                            println!(">>> Starting flash...");
                            flashing = true;
                        }
                        consecutive_a5_after_init = 0;
                    }
                }
            }
            Ok(_) => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                println!("ERROR reading: {}", e);
                return Err(e.into());
            }
        }
    }
}
