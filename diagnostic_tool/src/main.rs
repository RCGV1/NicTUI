use std::time::Duration;
use std::io::Read;

fn main() {
    println!("\n=== Simple Serial Monitor ===\n");
    
    let port_name = "/dev/tty.usbserial-1110";
    
    println!("Opening port: {}", port_name);
    
    let mut port = match serialport::new(port_name, 115200)
        .timeout(Duration::from_millis(50))
        .open() {
        Ok(p) => {
            println!("Port opened!");
            p
        }
        Err(e) => {
            println!("ERROR opening port: {}", e);
            return;
        }
    };
    
    println!("\n=== Listening for data (Ctrl+C to exit) ===");
    println!("Turn radio ON while holding PTT to see 0xA5 bytes...\n");
    
    let mut buf = [0u8; 128];
    let mut total = 0;
    
    loop {
        match port.read(&mut buf) {
            Ok(n) if n > 0 => {
                total += n;
                let hex: String = buf[..n].iter()
                    .map(|b| format!("{:02X} ", b))
                    .collect();
                let a5_count = buf[..n].iter().filter(|&&b| b == 0xA5).count();
                if a5_count == n {
                    println!("[total={}] {} bytes: {} (ALL 0xA5 - flash mode!)", total, n, hex.trim());
                } else if a5_count > 0 {
                    println!("[total={}] {} bytes: {} ({} are 0xA5)", total, n, hex.trim(), a5_count);
                } else {
                    println!("[total={}] {} bytes: {}", total, n, hex.trim());
                }
            }
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                println!("ERROR: {}", e);
                break;
            }
        }
    }
    
    println!("\nTotal bytes received: {}", total);
}
