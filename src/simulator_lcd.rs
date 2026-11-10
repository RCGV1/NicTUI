//! Optional read-only access to NicFWSim's asset-rendered native LCD.
//! Pixels are row-major BGR565 words; no UART, buttons or physical I/O is used.

use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

const BUDGET: Duration = Duration::from_millis(300);
const MAX_HEADER: usize = 8 * 1024;
const MAX_RESPONSE: usize = 256 * 1024;
const PIXELS: usize = 128 * 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    address: SocketAddr,
}

impl Endpoint {
    /// Numeric loopback HOST:PORT, optionally wrapped as http://HOST:PORT/.
    /// DNS, credentials, non-root paths, queries and fragments are not accepted.
    pub fn parse(input: &str) -> Result<Self> {
        let address = match input.strip_prefix("http://") {
            Some(address) => address.strip_suffix('/').unwrap_or(address),
            None => input,
        };
        let address: SocketAddr = address
            .parse()
            .context("Simulator LCD requires a numeric loopback HTTP address and port")?;
        ensure!(
            address.ip().is_loopback() && address.port() != 0,
            "Simulator LCD requires loopback and a nonzero port"
        );
        Ok(Self { address })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Fetch a complete 128x128 native LCD within one shared 300 ms I/O budget.
    /// Unsupported HTTP framing or absent/incomplete assets return an error.
    pub fn read_frame(&self) -> Result<Vec<u16>> {
        self.read_frame_inner(None)
    }

    /// Also require the HTTP backend to advertise the UART controlled by the app.
    /// This never connects to the UART endpoint or discovers other instances.
    pub fn read_frame_for(&self, expected_uart_uri: &str) -> Result<Vec<u16>> {
        let address: SocketAddr = expected_uart_uri
            .strip_prefix("sim://")
            .context("Simulator LCD binding requires a sim:// UART endpoint")?
            .parse()
            .context("Simulator LCD binding requires a numeric loopback UART endpoint")?;
        ensure!(
            address.ip().is_loopback() && address.port() != 0,
            "Simulator LCD binding requires loopback and a nonzero UART port"
        );
        self.read_frame_inner(Some(&format!("sim://{address}")))
    }

    fn read_frame_inner(&self, expected_uart_uri: Option<&str>) -> Result<Vec<u16>> {
        let deadline = Instant::now() + BUDGET;
        let mut stream = TcpStream::connect_timeout(&self.address, remaining(deadline)?)
            .context("Cannot connect to simulator LCD endpoint")?;
        stream.set_nodelay(true)?;
        let request = format!(
            "GET /api/state HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.address
        );
        let mut sent = 0;
        while sent < request.len() {
            stream.set_write_timeout(Some(remaining(deadline)?))?;
            match stream.write(&request.as_bytes()[sent..]) {
                Ok(0) => bail!("Simulator LCD request was truncated"),
                Ok(count) => sent += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("Simulator LCD request failed"),
            }
        }

        let mut response = Vec::with_capacity(MAX_HEADER);
        let header_end = loop {
            if let Some(index) = response.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break index + 4;
            }
            ensure!(
                response.len() < MAX_HEADER,
                "Simulator LCD HTTP header exceeds 8 KiB"
            );
            let mut buffer = [0; 4096];
            let limit = buffer.len().min(MAX_HEADER - response.len());
            let count = receive(&mut stream, &mut buffer[..limit], deadline)?;
            ensure!(count != 0, "Truncated simulator LCD HTTP header");
            response.extend_from_slice(&buffer[..count]);
        };
        let length = content_length(&response[..header_end])?;
        ensure!(
            length <= MAX_RESPONSE - header_end,
            "Simulator LCD response exceeds 256 KiB"
        );
        let total = header_end + length;
        ensure!(
            response.len() <= total,
            "Simulator LCD response exceeds Content-Length"
        );
        response.reserve(total - response.len());
        while response.len() < total {
            let mut buffer = [0; 4096];
            let limit = buffer.len().min(total - response.len());
            let count = receive(&mut stream, &mut buffer[..limit], deadline)?;
            ensure!(count != 0, "Truncated simulator LCD JSON body");
            response.extend_from_slice(&buffer[..count]);
        }
        let state: State = serde_json::from_slice(&response[header_end..])
            .context("Invalid simulator LCD JSON")?;
        remaining(deadline)?;
        ensure!(
            state.source == "nicfw-source-model" && state.simulated && state.version == 1,
            "Unrecognized simulator LCD source, version or simulation flag"
        );
        if let Some(expected) = expected_uart_uri {
            ensure!(
                state.uart_endpoint.as_deref() == Some(expected),
                "Simulator LCD backend does not match the controlled UART endpoint"
            );
        }
        let frame = state.framebuffer;
        ensure!(
            frame.width == 128 && frame.height == 128 && frame.pixels.len() == PIXELS,
            "Simulator LCD must contain exactly 128x128 pixels"
        );
        ensure!(
            frame.complete && frame.missing_glyphs.is_empty(),
            "Simulator LCD source assets are incomplete"
        );
        Ok(frame.pixels)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "http://{}", self.address)
    }
}

fn remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    ensure!(
        !remaining.is_zero(),
        "Simulator LCD exceeded its total time budget"
    );
    Ok(remaining)
}

fn receive(stream: &mut TcpStream, buffer: &mut [u8], deadline: Instant) -> Result<usize> {
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        match stream.read(buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result.context("Simulator LCD response read failed"),
        }
    }
}

fn content_length(header: &[u8]) -> Result<usize> {
    ensure!(header.is_ascii(), "Non-ASCII simulator LCD HTTP header");
    let header = std::str::from_utf8(header)?;
    let mut lines = header.split("\r\n");
    let mut status = lines
        .next()
        .context("Missing HTTP status")?
        .split_whitespace();
    ensure!(
        matches!(status.next(), Some("HTTP/1.1" | "HTTP/1.0")) && status.next() == Some("200"),
        "Simulator LCD requires HTTP 200"
    );
    let mut length = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .context("Malformed simulator LCD HTTP header")?;
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)),
            "Invalid simulator LCD HTTP header name"
        );
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            bail!("Simulator LCD Transfer-Encoding is unsupported");
        }
        if name.eq_ignore_ascii_case("content-encoding") {
            ensure!(
                value.eq_ignore_ascii_case("identity"),
                "Encoded simulator LCD body is unsupported"
            );
        }
        if name.eq_ignore_ascii_case("content-length") {
            ensure!(
                length.is_none()
                    && !value.is_empty()
                    && value.bytes().all(|byte| byte.is_ascii_digit()),
                "Invalid or duplicate simulator LCD Content-Length"
            );
            length = Some(
                value
                    .parse()
                    .context("Invalid simulator LCD Content-Length")?,
            );
        }
    }
    length.context("Simulator LCD requires Content-Length")
}

#[derive(Deserialize)]
struct State {
    source: String,
    simulated: bool,
    version: u8,
    uart_endpoint: Option<String>,
    framebuffer: Frame,
}

#[derive(Deserialize)]
struct Frame {
    width: u16,
    height: u16,
    complete: bool,
    pixels: Vec<u16>,
    #[serde(default)]
    missing_glyphs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::net::TcpListener;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::thread::{self, JoinHandle};

    struct Server {
        endpoint: Endpoint,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    fn wait(delay: Duration, stop: &AtomicBool) -> bool {
        let deadline = Instant::now() + delay;
        while Instant::now() < deadline {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
        !stop.load(Ordering::Relaxed)
    }

    impl Server {
        fn start(parts: Vec<(Duration, Vec<u8>)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = Endpoint::parse(&listener.local_addr().unwrap().to_string()).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = Arc::clone(&stop);
            let worker = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(1);
                let mut stream = loop {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    assert!(Instant::now() < deadline, "owned listener accept deadline");
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(50)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    assert!(Instant::now() < deadline && request.len() < 1024);
                    let mut byte = [0];
                    match stream.read(&mut byte) {
                        Ok(0) => return,
                        Ok(_) => request.push(byte[0]),
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Err(error) => panic!("{error}"),
                    }
                }
                assert_eq!(
                    request,
                    format!(
                        "GET /api/state HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                        endpoint.address()
                    )
                    .as_bytes()
                );
                for (delay, bytes) in parts {
                    if !wait(delay, &stopped) || stream.write_all(&bytes).is_err() {
                        return;
                    }
                }
            });
            Self {
                endpoint,
                stop,
                worker: Some(worker),
            }
        }

        fn response(bytes: Vec<u8>) -> Self {
            Self::start(vec![(Duration::ZERO, bytes)])
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    fn state() -> Value {
        json!({"source": "nicfw-source-model", "simulated": true, "version": 1,
            "framebuffer": {"width": 128, "height": 128, "complete": true,
                "pixels": vec![0xa7dfu16; PIXELS], "missing_glyphs": []}})
    }

    fn response(body: &[u8]) -> Vec<u8> {
        [
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes(),
            body.to_vec(),
        ]
        .concat()
    }

    #[test]
    fn parse_accepts_only_numeric_loopback_root_endpoints_without_connecting() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        for endpoint in [
            address.to_string(),
            format!("http://{address}"),
            format!("http://{address}/"),
        ] {
            assert_eq!(Endpoint::parse(&endpoint).unwrap().address(), address);
        }
        assert!(Endpoint::parse("http://[::1]:1234/").is_ok());
        for endpoint in [
            "",
            "localhost:1234",
            "http://localhost:1234/",
            "https://127.0.0.1:1234/",
            "http://127.0.0.1:1234/api/state",
            "127.0.0.1:1234?x",
            "http://127.0.0.1:1234/#x",
            "http://user@127.0.0.1:1234/",
            "127.0.0.1:0",
            "127.0.0.1",
            "0.0.0.0:1234",
            "192.0.2.1:1234",
            "[::]:1234",
            "http://[::ffff:192.0.2.1]:1234/",
            " 127.0.0.1:1234",
            "127.0.0.1:1234\r\nHost: localhost",
            "http://127.0.0.1:1234//",
        ] {
            assert!(Endpoint::parse(endpoint).is_err(), "{endpoint}");
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn get_returns_exact_native_words_and_does_not_need_eof_after_content_length() {
        let mut value = state();
        value["framebuffer"]["pixels"][0] = json!(0);
        value["framebuffer"]["pixels"][PIXELS - 1] = json!(65535);
        let server = Server::start(vec![
            (
                Duration::ZERO,
                response(&serde_json::to_vec(&value).unwrap()),
            ),
            (Duration::from_millis(450), Vec::new()),
        ]);
        let started = Instant::now();
        let pixels = server.endpoint.read_frame().unwrap();
        assert_eq!(pixels.len(), PIXELS);
        assert_eq!(pixels[0], 0);
        assert_eq!(pixels[1], 0xa7df);
        assert_eq!(pixels[PIXELS - 1], 65535);
        assert!(started.elapsed() < BUDGET);
    }

    #[test]
    fn rejects_http_errors_unsupported_framing_and_oversized_headers_or_bodies() {
        for bytes in [
            b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 500 Error\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/2 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999999\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\n\r\n{}".to_vec(),
            format!("HTTP/1.1 200 OK\r\nContent-Length: {MAX_RESPONSE}\r\n\r\n").into_bytes(),
            [
                b"HTTP/1.1 200 OK\r\nX-Padding: ".to_vec(),
                vec![b'x'; MAX_HEADER],
                b"\r\n\r\n".to_vec(),
            ]
            .concat(),
        ] {
            let server = Server::response(bytes);
            assert!(server.endpoint.read_frame().is_err());
        }
    }

    #[test]
    fn rejects_missing_untrusted_incomplete_or_malformed_frames_and_pixels() {
        let original = state();
        let mut invalid = vec![json!({}), json!(null)];
        for (pointer, replacement) in [
            ("/source", json!("untrusted")),
            ("/simulated", json!(false)),
            ("/simulated", json!(1)),
            ("/version", json!(2)),
            ("/version", json!(1.0)),
            ("/framebuffer", json!(null)),
            ("/framebuffer/width", json!(127)),
            ("/framebuffer/height", json!(129)),
            ("/framebuffer/complete", json!(false)),
            ("/framebuffer/pixels", json!([])),
            ("/framebuffer/pixels/0", json!(65536)),
            ("/framebuffer/pixels/0", json!(-1)),
            ("/framebuffer/pixels/0", json!(1.5)),
            ("/framebuffer/pixels/0", json!("0")),
            ("/framebuffer/missing_glyphs", json!(["missing-symbol"])),
        ] {
            let mut value = original.clone();
            *value.pointer_mut(pointer).unwrap() = replacement;
            invalid.push(value);
        }
        for value in invalid {
            let server = Server::response(response(&serde_json::to_vec(&value).unwrap()));
            assert!(server.endpoint.read_frame().is_err());
        }
        let server = Server::response(response(b"{not JSON}"));
        assert!(server.endpoint.read_frame().is_err());
    }

    #[test]
    fn rejects_truncation_and_applies_one_deadline_to_header_and_body_drips() {
        for bytes in [
            b"HTTP/1.1 200 OK\r\nContent-Len".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 500\r\n\r\n{}".to_vec(),
        ] {
            assert!(Server::response(bytes).endpoint.read_frame().is_err());
        }
        let bytes = response(&serde_json::to_vec(&state()).unwrap());
        let parts = bytes
            .chunks(bytes.len().div_ceil(5))
            .map(|bytes| (Duration::from_millis(90), bytes.to_vec()))
            .collect();
        let server = Server::start(parts);
        let started = Instant::now();
        assert!(server.endpoint.read_frame().is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
        let server = Server::start(vec![(Duration::from_millis(450), response(b"{}"))]);
        let started = Instant::now();
        assert!(server.endpoint.read_frame().is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn bound_frame_requires_same_instance_and_never_connects_to_uart() {
        let uart = TcpListener::bind("127.0.0.1:0").unwrap();
        uart.set_nonblocking(true).unwrap();
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        let expected = format!("sim://{}", uart.local_addr().unwrap());
        let mut value = state();
        value["uart_endpoint"] = json!(expected);
        let server = Server::response(response(&serde_json::to_vec(&value).unwrap()));
        assert_eq!(
            server.endpoint.read_frame_for(&expected).unwrap().len(),
            PIXELS
        );
        assert_eq!(
            uart.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );

        for endpoint in [
            json!(null),
            json!(format!("sim://{}", other.local_addr().unwrap())),
            json!("sim://127.0.0.1:"),
            json!("sim://127.0.0.1:0"),
            json!("sim://localhost:1234"),
            json!("sim://192.0.2.1:1234"),
            json!(1234),
        ] {
            let mut value = state();
            value["uart_endpoint"] = endpoint;
            let server = Server::response(response(&serde_json::to_vec(&value).unwrap()));
            assert!(server.endpoint.read_frame_for(&expected).is_err());
        }
        let server = Server::response(response(&serde_json::to_vec(&state()).unwrap()));
        assert!(server.endpoint.read_frame_for(&expected).is_err());
        assert_eq!(
            uart.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn invalid_uart_binding_is_rejected_before_http_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = Endpoint::parse(&listener.local_addr().unwrap().to_string()).unwrap();
        for uart in [
            "127.0.0.1:1",
            "sim://localhost:1",
            "sim://127.0.0.1:0",
            "sim://192.0.2.1:1",
            "sim://127.0.0.1:",
            "sim://127.0.0.1:1/path",
        ] {
            assert!(endpoint.read_frame_for(uart).is_err(), "{uart}");
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
