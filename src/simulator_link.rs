//! Connector for the separately installed NicFWSim local UART endpoint.
//! This module contains no simulated radio implementation or physical I/O.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};

pub const PREFIX: &str = "sim://";

pub fn parse_endpoint(uri: &str) -> Result<SocketAddr> {
    let endpoint = uri
        .strip_prefix(PREFIX)
        .context("Simulator targets must use sim://")?;
    let address: SocketAddr = endpoint
        .parse()
        .context("Use a numeric loopback address and port, such as sim://127.0.0.1:8788")?;
    ensure!(
        address.ip().is_loopback() && address.port() != 0,
        "Simulator connections must use a loopback address and a nonzero port"
    );
    Ok(address)
}

pub struct SimulatorLink {
    stream: TcpStream,
}

impl SimulatorLink {
    pub fn connect(uri: &str) -> Result<Self> {
        let address = parse_endpoint(uri)?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(1)).context(
            "Cannot connect to NicFWSim; start the separately installed nicfw-sim add-on first",
        )?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        Ok(Self { stream })
    }

    pub fn clear(&mut self, clear: serialport::ClearBuffer) -> io::Result<()> {
        if matches!(clear, serialport::ClearBuffer::Output) {
            return Ok(());
        }
        self.stream.set_nonblocking(true)?;
        let deadline = Instant::now() + Duration::from_millis(20);
        let result = (|| {
            let mut buffer = [0; 4096];
            for _ in 0..16 {
                if Instant::now() >= deadline {
                    break;
                }
                match self.stream.read(&mut buffer) {
                    Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        })();
        let restored = self.stream.set_nonblocking(false);
        result.and(restored)
    }
}

impl Read for SimulatorLink {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        match self.stream.read(buffer) {
            Ok(0) => Err(io::ErrorKind::UnexpectedEof.into()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Err(io::ErrorKind::TimedOut.into())
            }
            result => result,
        }
    }
}

impl Write for SimulatorLink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.write(buffer)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn only_literal_loopback_nonzero_endpoints_are_accepted() {
        for endpoint in ["sim://127.0.0.1:8788", "sim://[::1]:8788"] {
            assert!(parse_endpoint(endpoint).is_ok());
        }
        for endpoint in [
            "/dev/cu.radio",
            "ble://radio",
            "sim://localhost:8788",
            "sim://0.0.0.0:8788",
            "sim://192.0.2.1:8788",
            "sim://127.0.0.1:0",
            "sim://invalid",
        ] {
            assert!(parse_endpoint(endpoint).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn connector_roundtrips_uart_and_reports_eof() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut bytes = [0; 3];
            peer.read_exact(&mut bytes).unwrap();
            assert_eq!(bytes, [0x01, 0x4b, 0x4a]);
            peer.write_all(&bytes).unwrap();
        });
        let mut link = SimulatorLink::connect(&format!("sim://{address}")).unwrap();
        link.write_all(&[0x01, 0x4b, 0x4a]).unwrap();
        let mut response = [0; 3];
        link.read_exact(&mut response).unwrap();
        assert_eq!(response, [0x01, 0x4b, 0x4a]);
        worker.join().unwrap();
        assert_eq!(
            link.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
