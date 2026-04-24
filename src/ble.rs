use std::collections::VecDeque;
use std::env;
use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::sync::mpsc::{self as std_mpsc, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::stream::StreamExt;
use serde::Serialize;
use serialport::ClearBuffer;
use tokio::runtime::Builder;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

pub const TD_H3_BLE_SERVICE_UUID: &str = "0000ff00-0000-1000-8000-00805f9b34fb";
pub const TD_H3_BLE_READ_UUID: &str = "0000ff01-0000-1000-8000-00805f9b34fb";
pub const TD_H3_BLE_WRITE_UUID: &str = "0000ff02-0000-1000-8000-00805f9b34fb";
const DEFAULT_SCAN_TIMEOUT: Duration = Duration::from_secs(8);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MACOS_ADAPTER_ACQUISITION_TIMEOUT: Duration = Duration::from_secs(25);
const DEFAULT_ADAPTER_ACQUISITION_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_GATT_WRITE_CHUNK_SIZE: usize = 20;

fn ble_debug_enabled() -> bool {
    env::var_os("NICTUI_BLE_DEBUG").is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn ble_log(message: impl AsRef<str>) {
    if ble_debug_enabled() {
        eprintln!("[ble] {}", message.as_ref());
    }
}

fn ble_debug_log(message: impl AsRef<str>) {
    ble_log(message);
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BleDevice {
    pub device_id: String,
    pub name: Option<String>,
    pub rssi: Option<i32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BleBridgeStatus {
    pub device_id: String,
    pub tty_path: String,
    pub pid: Option<u32>,
    pub active: bool,
    pub reused_existing: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BleReadinessKind {
    Ready,
    PermissionBlocked,
    RuntimeBlocked,
    DiscoveryEmpty,
    TargetNotFound,
    TargetAmbiguous,
    ConnectivityFailed,
    ProtocolMismatch,
}

impl fmt::Display for BleReadinessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Ready => "ready",
            Self::PermissionBlocked => "permission-blocked",
            Self::RuntimeBlocked => "runtime-blocked",
            Self::DiscoveryEmpty => "discovery-empty",
            Self::TargetNotFound => "target-not-found",
            Self::TargetAmbiguous => "target-ambiguous",
            Self::ConnectivityFailed => "connectivity-failed",
            Self::ProtocolMismatch => "protocol-mismatch",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BleReadinessReport {
    pub ok: bool,
    pub kind: BleReadinessKind,
    pub stage: String,
    pub summary: String,
    pub next_action: String,
    pub target: Option<String>,
    pub device: Option<BleDevice>,
}

impl BleReadinessReport {
    pub fn detail(&self) -> String {
        format!("{} Next action: {}", self.summary, self.next_action)
    }
}

#[derive(Debug, Clone)]
pub enum BleTarget {
    Device(String),
    Name(String),
}

impl BleTarget {
    pub fn display(&self) -> &str {
        match self {
            BleTarget::Device(value) | BleTarget::Name(value) => value.as_str(),
        }
    }
}

pub struct BleLink {
    peripheral: Peripheral,
    write_characteristic: Characteristic,
    notify_characteristic: Characteristic,
    write_type: WriteType,
    notifications_rx: Receiver<Vec<u8>>,
    pending_bytes: VecDeque<u8>,
    read_timeout: Duration,
}

impl BleLink {
    pub fn connect(device_id: &str) -> Result<Self> {
        let device_id = device_id.trim().to_string();
        ble_log(format!("connect start device={}", device_id));
        let connect_device_id = device_id.clone();
        let (peripheral, write_characteristic, notify_characteristic, write_type, notifications_rx) =
            block_on_ble_runtime(move |state| {
                Box::pin(async move { connect_td_h3_ble(state, &connect_device_id).await })
            })?;
        ble_log(format!("connect ready device={}", device_id));

        Ok(Self {
            peripheral,
            write_characteristic,
            notify_characteristic,
            write_type,
            notifications_rx,
            pending_bytes: VecDeque::new(),
            read_timeout: Duration::from_millis(50),
        })
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.read_timeout = timeout;
    }

    pub fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        ble_debug_log(format!("write {} byte(s)", data.len()));
        let peripheral = self.peripheral.clone();
        let write_characteristic = self.write_characteristic.clone();
        let write_type = self.write_type;
        let chunks = chunk_gatt_write_payload(data)
            .into_iter()
            .map(Vec::from)
            .collect::<Vec<_>>();
        block_on_ble_runtime(move |_| {
            Box::pin(async move {
                for chunk in chunks {
                    peripheral
                        .write(&write_characteristic, &chunk, write_type)
                        .await
                        .map_err(map_ble_error)?;
                }
                Ok(())
            })
        })
        .map_err(io_error_passthrough)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    pub fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }

        let deadline = Instant::now() + self.read_timeout;
        while self.pending_bytes.is_empty() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "BLE read timed out",
                ));
            };

            match self.notifications_rx.recv_timeout(remaining) {
                Ok(chunk) => self.pending_bytes.extend(chunk),
                Err(RecvTimeoutError::Timeout) => {
                    ble_debug_log("read timed out waiting for notifications");
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "BLE read timed out",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    ble_log("notification stream ended unexpectedly");
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "BLE notification stream ended",
                    ));
                }
            }
        }

        while let Ok(chunk) = self.notifications_rx.try_recv() {
            self.pending_bytes.extend(chunk);
        }

        let mut read = 0usize;
        while read < buffer.len() {
            let Some(byte) = self.pending_bytes.pop_front() else {
                break;
            };
            buffer[read] = byte;
            read += 1;
        }

        Ok(read)
    }

    pub fn clear(&mut self, clear: ClearBuffer) -> io::Result<()> {
        ble_debug_log(format!("clear buffers mode={clear:?}"));
        match clear {
            ClearBuffer::Input | ClearBuffer::All => {
                self.pending_bytes.clear();
                while self.notifications_rx.try_recv().is_ok() {}
            }
            ClearBuffer::Output => {}
        }
        Ok(())
    }
}

impl Drop for BleLink {
    fn drop(&mut self) {
        let peripheral = self.peripheral.clone();
        let notify_characteristic = self.notify_characteristic.clone();
        let _ = block_on_ble_runtime(move |_| {
            Box::pin(async move {
                let _ = peripheral.unsubscribe(&notify_characteristic).await;
                let _ = peripheral.disconnect().await;
                Ok(())
            })
        });
    }
}

pub fn default_scan_timeout() -> Duration {
    DEFAULT_SCAN_TIMEOUT
}

pub fn ble_scan_supported() -> bool {
    cfg!(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    ))
}

pub fn scan_td_h3_ble_devices(timeout: Duration) -> Result<Vec<BleDevice>> {
    ble_log(format!("scan requested timeout={}s", timeout.as_secs()));
    let devices =
        block_on_ble_runtime(move |state| Box::pin(scan_td_h3_ble_devices_async(state, timeout)))?;
    ble_log(format!("scan complete devices={}", devices.len()));
    Ok(devices)
}

pub fn resolve_ble_target(target: &BleTarget, timeout: Duration) -> Result<BleDevice> {
    ble_log(format!(
        "resolve target={} timeout={}s",
        target.display(),
        timeout.as_secs()
    ));
    match target {
        BleTarget::Device(device_id) => {
            ble_log(format!(
                "resolve using explicit device={}",
                device_id.trim()
            ));
            Ok(BleDevice {
                device_id: device_id.trim().to_string(),
                name: None,
                rssi: None,
            })
        }
        BleTarget::Name(name) => {
            let normalized = normalize_ble_name(name);
            let matches = scan_td_h3_ble_devices(timeout)?
                .into_iter()
                .filter(|device| {
                    device
                        .name
                        .as_deref()
                        .map(normalize_ble_name)
                        .is_some_and(|value| value == normalized)
                })
                .collect::<Vec<_>>();

            match matches.as_slice() {
                [] => bail!("No BLE radio named '{}' was found.", name),
                [single] => {
                    ble_log(format!(
                        "resolve matched name={} device={}",
                        name, single.device_id
                    ));
                    Ok(single.clone())
                }
                _ => bail!(
                    "More than one BLE radio matched '{}'. Use --ble-device with an explicit UUID.",
                    name
                ),
            }
        }
    }
}

pub fn ensure_ble_bridge(target: &BleTarget, timeout: Duration) -> Result<BleBridgeStatus> {
    ble_log(format!("bridge ensure target={}", target.display()));
    let device = resolve_ble_target(target, timeout)?;
    ensure_ble_bridge_for_device(&device.device_id)
}

pub fn ensure_ble_bridge_for_device(device_id: &str) -> Result<BleBridgeStatus> {
    let device_id = device_id.trim();
    let link = BleLink::connect(device_id)
        .with_context(|| format!("Failed to validate BLE connection for {device_id}"))?;
    drop(link);
    ble_log(format!("bridge ready device={device_id}"));
    Ok(BleBridgeStatus {
        device_id: device_id.to_string(),
        tty_path: ble_device_uri(device_id),
        pid: None,
        active: true,
        reused_existing: false,
    })
}

pub fn disconnect_ble_bridge_for_device(device_id: &str) -> Result<BleBridgeStatus> {
    Ok(BleBridgeStatus {
        device_id: device_id.trim().to_string(),
        tty_path: ble_device_uri(device_id),
        pid: None,
        active: false,
        reused_existing: false,
    })
}

pub fn ble_device_uri(device_id: &str) -> String {
    format!("ble://{}", device_id.trim())
}

pub fn parse_ble_device_uri(value: &str) -> Option<&str> {
    value
        .strip_prefix("ble://")
        .or_else(|| value.strip_prefix("ble:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn assess_ble_readiness(target: Option<&BleTarget>, timeout: Duration) -> BleReadinessReport {
    let target_label = target.map(|target| target.display().to_string());
    match scan_td_h3_ble_devices(timeout) {
        Ok(devices) => assess_ble_readiness_from_devices(devices, target),
        Err(error) => classify_ble_operation_failure("scan", error, target_label, None),
    }
}

async fn scan_td_h3_ble_devices_async(
    state: &mut BleRuntimeState,
    timeout: Duration,
) -> Result<Vec<BleDevice>> {
    ble_log("adapter discovery start");
    let adapter = state.adapter().await?;
    ble_log("adapter discovery complete");
    ble_log("scan start");
    adapter
        .start_scan(ScanFilter {
            services: vec![td_h3_service_uuid()?],
        })
        .await
        .context("Failed to start BLE scan")?;

    tokio::time::sleep(timeout).await;
    ble_log("scan collecting peripherals");

    let mut devices = Vec::new();
    for peripheral in adapter
        .peripherals()
        .await
        .context("Failed to list BLE devices")?
    {
        let Some(properties) = peripheral
            .properties()
            .await
            .context("Failed to read BLE device properties")?
        else {
            continue;
        };

        if !properties
            .services
            .iter()
            .any(|uuid| *uuid == td_h3_service_uuid().unwrap())
            && !properties
                .local_name
                .as_deref()
                .is_some_and(|name| normalize_ble_name(name).contains("tdh3"))
        {
            continue;
        }

        ble_debug_log(format!(
            "scan match device={} name={} rssi={:?}",
            peripheral.id(),
            properties.local_name.as_deref().unwrap_or("<unknown>"),
            properties.rssi
        ));
        devices.push(BleDevice {
            device_id: peripheral.id().to_string(),
            name: properties.local_name,
            rssi: properties.rssi.map(i32::from),
        });
    }

    devices.sort_by(|left, right| right.rssi.cmp(&left.rssi));
    devices.dedup_by(|left, right| left.device_id == right.device_id);
    Ok(devices)
}

type BleRuntimeTask = Box<
    dyn for<'a> FnOnce(&'a mut BleRuntimeState) -> Pin<Box<dyn Future<Output = ()> + 'a>> + Send,
>;

struct BleRuntimeHandle {
    sender: UnboundedSender<BleRuntimeTask>,
}

struct BleRuntimeState {
    adapter: Option<Adapter>,
}

impl BleRuntimeState {
    fn new() -> Self {
        Self { adapter: None }
    }

    async fn adapter(&mut self) -> Result<Adapter> {
        if let Some(adapter) = self.adapter.clone() {
            ble_debug_log("adapter cache hit");
            return Ok(adapter);
        }

        ble_log("manager open start");
        let manager = Manager::new().await.context("Failed to open BLE manager")?;
        ble_log("manager open complete");
        ble_log("adapter list start");
        let timeout = adapter_acquisition_timeout();
        let mut adapters = tokio::time::timeout(timeout, manager.adapters())
            .await
            .with_context(|| ble_adapter_timeout_message(timeout))?
            .context("Failed to list BLE adapters")?;
        let adapter = adapters
            .drain(..)
            .next()
            .ok_or_else(|| anyhow!("No BLE adapters are available on this system"))?;
        ble_log("adapter list complete");
        self.adapter = Some(adapter.clone());
        Ok(adapter)
    }
}

fn adapter_acquisition_timeout() -> Duration {
    if cfg!(target_os = "macos") {
        MACOS_ADAPTER_ACQUISITION_TIMEOUT
    } else {
        DEFAULT_ADAPTER_ACQUISITION_TIMEOUT
    }
}

impl BleRuntimeHandle {
    fn new() -> Result<Self> {
        let (sender, receiver) = unbounded_channel();
        thread::Builder::new()
            .name("nictui-ble-runtime".to_string())
            .spawn(move || run_ble_worker(receiver))
            .context("Failed to spawn BLE runtime thread")?;
        Ok(Self { sender })
    }
}

fn run_ble_worker(mut receiver: UnboundedReceiver<BleRuntimeTask>) {
    let runtime = match Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            ble_log(format!("BLE runtime worker failed to start: {error}"));
            return;
        }
    };
    runtime.block_on(async move {
        let mut state = BleRuntimeState::new();
        // Keep the current-thread runtime alive for the full process so CoreBluetooth-backed
        // tasks spawned from btleplug continue to make progress on macOS.
        while let Some(task) = receiver.recv().await {
            task(&mut state).await;
        }
    });
}

fn ble_runtime() -> Result<&'static BleRuntimeHandle> {
    static BLE_RUNTIME: OnceLock<Result<BleRuntimeHandle, String>> = OnceLock::new();
    BLE_RUNTIME
        .get_or_init(|| BleRuntimeHandle::new().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| anyhow!("{error}"))
}

fn block_on_ble_runtime<T>(
    task: impl for<'a> FnOnce(&'a mut BleRuntimeState) -> Pin<Box<dyn Future<Output = Result<T>> + 'a>>
    + Send
    + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let (result_tx, result_rx) = std_mpsc::sync_channel(1);
    ble_runtime()?
        .sender
        .send(Box::new(move |state| {
            Box::pin(async move {
                let result = task(state).await;
                let _ = result_tx.send(result);
            })
        }))
        .map_err(|_| anyhow!("BLE runtime worker is unavailable"))?;
    result_rx
        .recv()
        .map_err(|_| anyhow!("BLE runtime worker stopped before returning a result"))?
}

fn ble_adapter_timeout_message(timeout: Duration) -> String {
    if cfg!(target_os = "macos") {
        format!(
            "Timed out after {}s waiting for BLE adapter enumeration. On macOS this usually means CoreBluetooth never delivered its initial state update. A common cause is Bluetooth/TCC permission being attributed to a hosted wrapper instead of NicTUI itself. If you launched NicTUI from Codex or another hosted wrapper, verify the app bundle includes NSBluetoothAlwaysUsageDescription, then launch the built NicTUI.app outside hosted wrappers, such as from Finder or a normal Terminal session, and retry.",
            timeout.as_secs()
        )
    } else {
        format!(
            "Timed out after {}s waiting for BLE adapter enumeration.",
            timeout.as_secs()
        )
    }
}

fn io_error_passthrough(error: anyhow::Error) -> io::Error {
    match error.downcast::<io::Error>() {
        Ok(error) => error,
        Err(error) => io::Error::other(error.to_string()),
    }
}

async fn connect_td_h3_ble(
    state: &mut BleRuntimeState,
    device_id: &str,
) -> Result<(
    Peripheral,
    Characteristic,
    Characteristic,
    WriteType,
    Receiver<Vec<u8>>,
)> {
    ble_log(format!(
        "connect phase adapter discovery device={}",
        device_id.trim()
    ));
    let adapter = state.adapter().await?;
    ble_log("connect phase adapter ready");
    ble_log("connect phase scan start");
    adapter
        .start_scan(ScanFilter {
            services: vec![td_h3_service_uuid()?],
        })
        .await
        .context("Failed to start BLE scan")?;

    ble_log("connect phase waiting for peripheral");
    let peripheral = wait_for_peripheral(&adapter, device_id, DEFAULT_CONNECT_TIMEOUT).await?;
    ble_log(format!(
        "connect phase peripheral found id={}",
        peripheral.id()
    ));
    if !peripheral
        .is_connected()
        .await
        .context("Failed to check BLE connection state")?
    {
        ble_log("connect phase connect start");
        peripheral
            .connect()
            .await
            .context("Failed to connect over BLE")?;
        ble_log("connect phase connect complete");
    } else {
        ble_log("connect phase peripheral already connected");
    }
    ble_log("connect phase discover services start");
    peripheral
        .discover_services()
        .await
        .context("Failed to discover BLE services")?;
    ble_log("connect phase discover services complete");

    let notify_characteristic = peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| characteristic.uuid == td_h3_read_uuid().unwrap())
        .ok_or_else(|| {
            anyhow!(
                "TD-H3 BLE notify characteristic {} not found",
                TD_H3_BLE_READ_UUID
            )
        })?;
    ble_log(format!(
        "connect phase notify characteristic {} ready",
        notify_characteristic.uuid
    ));
    let write_characteristic = peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| characteristic.uuid == td_h3_write_uuid().unwrap())
        .ok_or_else(|| {
            anyhow!(
                "TD-H3 BLE write characteristic {} not found",
                TD_H3_BLE_WRITE_UUID
            )
        })?;
    ble_log(format!(
        "connect phase write characteristic {} ready",
        write_characteristic.uuid
    ));

    let write_type = if write_characteristic
        .properties
        .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
    {
        WriteType::WithoutResponse
    } else {
        WriteType::WithResponse
    };
    ble_log(format!("connect phase write mode {:?}", write_type));

    ble_log("connect phase notification stream open start");
    let mut notifications = peripheral
        .notifications()
        .await
        .context("Failed to open BLE notification stream")?;
    ble_log("connect phase notification stream open complete");
    ble_log("connect phase subscribe start");
    peripheral
        .subscribe(&notify_characteristic)
        .await
        .context("Failed to subscribe to TD-H3 BLE notifications")?;
    ble_log("connect phase subscribe complete");

    let expected_uuid = notify_characteristic.uuid;
    let (tx, rx) = std_mpsc::channel();
    tokio::spawn(async move {
        while let Some(notification) = notifications.next().await {
            ble_debug_log(format!(
                "notification uuid={} bytes={}",
                notification.uuid,
                notification.value.len()
            ));
            if notification.uuid == expected_uuid && tx.send(notification.value).is_err() {
                ble_debug_log("notification receiver dropped");
                break;
            }
        }
    });

    Ok((
        peripheral,
        write_characteristic,
        notify_characteristic,
        write_type,
        rx,
    ))
}

async fn wait_for_peripheral(
    adapter: &Adapter,
    device_id: &str,
    timeout: Duration,
) -> Result<Peripheral> {
    let normalized = device_id.trim();
    let deadline = Instant::now() + timeout;
    ble_log(format!(
        "wait peripheral start device={} timeout={}s",
        normalized,
        timeout.as_secs()
    ));
    loop {
        for peripheral in adapter
            .peripherals()
            .await
            .context("Failed to list BLE devices")?
        {
            if peripheral.id().to_string().eq_ignore_ascii_case(normalized) {
                ble_log(format!("wait peripheral matched device={}", normalized));
                return Ok(peripheral);
            }
        }

        if Instant::now() >= deadline {
            bail!("Timed out waiting for BLE radio {}", device_id);
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn td_h3_service_uuid() -> Result<Uuid> {
    Uuid::parse_str(TD_H3_BLE_SERVICE_UUID).context("Invalid TD-H3 BLE service UUID")
}

fn td_h3_read_uuid() -> Result<Uuid> {
    Uuid::parse_str(TD_H3_BLE_READ_UUID).context("Invalid TD-H3 BLE read UUID")
}

fn td_h3_write_uuid() -> Result<Uuid> {
    Uuid::parse_str(TD_H3_BLE_WRITE_UUID).context("Invalid TD-H3 BLE write UUID")
}

fn normalize_ble_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn map_ble_error(error: btleplug::Error) -> io::Error {
    io::Error::other(error.to_string())
}

fn chunk_gatt_write_payload(data: &[u8]) -> Vec<&[u8]> {
    data.chunks(DEFAULT_GATT_WRITE_CHUNK_SIZE).collect()
}

fn assess_ble_readiness_from_devices(
    devices: Vec<BleDevice>,
    target: Option<&BleTarget>,
) -> BleReadinessReport {
    match target {
        None => {
            if devices.is_empty() {
                BleReadinessReport {
                    ok: false,
                    kind: BleReadinessKind::DiscoveryEmpty,
                    stage: "scan".to_string(),
                    summary:
                        "BLE discovery completed, but no TD-H3 radios were visible during the scan."
                            .to_string(),
                    next_action: "Keep the radio awake and nearby, enable Bluetooth on the radio over USB if needed, then rerun `nictui bluetooth doctor` or `nictui bluetooth scan`.".to_string(),
                    target: None,
                    device: None,
                }
            } else {
                let strongest = devices
                    .first()
                    .map(describe_ble_device)
                    .unwrap_or_else(|| "TD-H3 radio".to_string());
                BleReadinessReport {
                    ok: true,
                    kind: BleReadinessKind::Ready,
                    stage: "scan".to_string(),
                    summary: format!(
                        "BLE discovery is working and found {} TD-H3 radio(s); strongest match: {}.",
                        devices.len(),
                        strongest
                    ),
                    next_action: "Open a specific target with `nictui bluetooth doctor --device <uuid>` or continue with `nictui --ble-device <uuid> ...`.".to_string(),
                    target: None,
                    device: devices.first().cloned(),
                }
            }
        }
        Some(BleTarget::Name(name)) => {
            let normalized = normalize_ble_name(name);
            let matches = devices
                .into_iter()
                .filter(|device| {
                    device
                        .name
                        .as_deref()
                        .map(normalize_ble_name)
                        .is_some_and(|value| value == normalized)
                })
                .collect::<Vec<_>>();

            match matches.as_slice() {
                [] => BleReadinessReport {
                    ok: false,
                    kind: BleReadinessKind::TargetNotFound,
                    stage: "resolve".to_string(),
                    summary: format!(
                        "BLE discovery is working, but no TD-H3 radio named '{}' was found.",
                        name
                    ),
                    next_action:
                        "Run `nictui bluetooth scan` to confirm the advertised name or rerun with `--device <uuid>`."
                            .to_string(),
                    target: Some(name.to_string()),
                    device: None,
                },
                [single] => assess_ble_target_connection(single.clone(), Some(name.to_string())),
                _ => BleReadinessReport {
                    ok: false,
                    kind: BleReadinessKind::TargetAmbiguous,
                    stage: "resolve".to_string(),
                    summary: format!(
                        "BLE discovery found {} radios matching '{}'.",
                        matches.len(),
                        name
                    ),
                    next_action:
                        "Run `nictui bluetooth scan` and rerun with `--device <uuid>` so NicTUI opens the exact radio you want."
                            .to_string(),
                    target: Some(name.to_string()),
                    device: None,
                },
            }
        }
        Some(BleTarget::Device(device_id)) => {
            let device = devices
                .into_iter()
                .find(|device| device.device_id.eq_ignore_ascii_case(device_id.trim()))
                .unwrap_or_else(|| BleDevice {
                    device_id: device_id.trim().to_string(),
                    name: None,
                    rssi: None,
                });
            assess_ble_target_connection(device, Some(device_id.trim().to_string()))
        }
    }
}

fn assess_ble_target_connection(device: BleDevice, target: Option<String>) -> BleReadinessReport {
    match BleLink::connect(&device.device_id) {
        Ok(_) => BleReadinessReport {
            ok: true,
            kind: BleReadinessKind::Ready,
            stage: "connect".to_string(),
            summary: format!(
                "BLE discovery and TD-H3 transport open both succeeded for {}.",
                describe_ble_device(&device)
            ),
            next_action:
                "Run your NicTUI command against this BLE target; if the later handshake fails, treat that as a radio/protocol issue rather than a macOS Bluetooth permission issue."
                    .to_string(),
            target,
            device: Some(device),
        },
        Err(error) => classify_ble_operation_failure("connect", error, target, Some(device)),
    }
}

fn classify_ble_operation_failure(
    stage: &str,
    error: anyhow::Error,
    target: Option<String>,
    device: Option<BleDevice>,
) -> BleReadinessReport {
    let message = error.to_string();
    let kind = classify_ble_failure_kind(&message, stage);
    let summary = match kind {
        BleReadinessKind::PermissionBlocked => format!(
            "BLE {} did not complete because macOS likely blocked or misattributed Bluetooth permission: {}",
            stage, message
        ),
        BleReadinessKind::RuntimeBlocked => {
            format!(
                "BLE {} failed before the radio link came up: {}",
                stage, message
            )
        }
        BleReadinessKind::ProtocolMismatch => format!(
            "BLE connected far enough to reach the radio, but the expected TD-H3 GATT shape was missing: {}",
            message
        ),
        BleReadinessKind::ConnectivityFailed => format!(
            "BLE discovery worked, but the target transport still failed during {}: {}",
            stage, message
        ),
        _ => format!("BLE {} failed: {}", stage, message),
    };
    let next_action = match kind {
        BleReadinessKind::PermissionBlocked => "Launch the built NicTUI.app directly from Finder or a normal Terminal session outside hosted wrappers, allow Bluetooth for NicTUI, then rerun this doctor.".to_string(),
        BleReadinessKind::RuntimeBlocked => "Verify the Mac Bluetooth adapter is available and powered on, then retry. If you are running inside a hosted wrapper, retry from the built NicTUI.app or a normal Terminal session.".to_string(),
        BleReadinessKind::ProtocolMismatch => "Confirm the radio is advertising the TD-H3 BLE service FF00 with notify/read FF01 and write FF02, then retry.".to_string(),
        BleReadinessKind::ConnectivityFailed => "Keep the radio awake and nearby, power-cycle the radio's Bluetooth setting if needed, and retry. If scan keeps working but connect fails, treat this as a radio/connectivity problem rather than a macOS permission problem.".to_string(),
        _ => "Retry the BLE readiness check.".to_string(),
    };

    BleReadinessReport {
        ok: false,
        kind,
        stage: stage.to_string(),
        summary,
        next_action,
        target,
        device,
    }
}

fn classify_ble_failure_kind(message: &str, stage: &str) -> BleReadinessKind {
    if is_macos_permission_block(message) {
        BleReadinessKind::PermissionBlocked
    } else if message.contains("No BLE adapters are available")
        || message.contains("Failed to open BLE manager")
        || message.contains("Failed to list BLE adapters")
        || (stage == "scan" && message.contains("Failed to start BLE scan"))
    {
        BleReadinessKind::RuntimeBlocked
    } else if message.contains("characteristic") {
        BleReadinessKind::ProtocolMismatch
    } else {
        BleReadinessKind::ConnectivityFailed
    }
}

fn is_macos_permission_block(message: &str) -> bool {
    message.contains("Bluetooth/TCC permission")
        || message.contains("CoreBluetooth never delivered its initial state update")
        || message.contains("hosted wrapper")
        || message.contains("NSBluetoothAlwaysUsageDescription")
        || message.contains("NicTUI.app outside hosted wrappers")
}

pub fn ble_error_suggests_permission_block(message: &str) -> bool {
    is_macos_permission_block(message)
}

fn describe_ble_device(device: &BleDevice) -> String {
    match (device.name.as_deref(), device.rssi) {
        (Some(name), Some(rssi)) => format!("{} ({}, rssi={})", device.device_id, name, rssi),
        (Some(name), None) => format!("{} ({})", device.device_id, name),
        (None, Some(rssi)) => format!("{} (rssi={})", device.device_id, rssi),
        (None, None) => device.device_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::anyhow;

    use super::{
        BleDevice, BleReadinessKind, BleTarget, adapter_acquisition_timeout,
        assess_ble_readiness_from_devices, ble_adapter_timeout_message, ble_debug_enabled,
        ble_device_uri, classify_ble_failure_kind, classify_ble_operation_failure,
        normalize_ble_name, parse_ble_device_uri,
    };

    #[test]
    fn ble_uri_round_trip() {
        let uri = ble_device_uri("12345678-1234-5678-9ABC-DEF012345678");
        assert_eq!(
            parse_ble_device_uri(&uri),
            Some("12345678-1234-5678-9ABC-DEF012345678")
        );
    }

    #[test]
    fn normalize_ble_name_ignores_spacing_and_case() {
        assert_eq!(normalize_ble_name("TD-H3"), normalize_ble_name("td h3"));
    }

    #[test]
    fn gatt_write_payloads_are_split_into_twenty_byte_chunks() {
        let payload = (0..45).collect::<Vec<_>>();
        let chunks = super::chunk_gatt_write_payload(&payload);

        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).collect::<Vec<_>>(),
            vec![20, 20, 5]
        );
        assert_eq!(chunks.concat(), payload);
    }

    #[test]
    fn gatt_write_payloads_allow_common_twenty_byte_packets() {
        let payload = [0xA5; 20];
        let chunks = super::chunk_gatt_write_payload(&payload);

        assert_eq!(chunks, vec![payload.as_slice()]);
    }

    #[test]
    fn ble_debug_env_gate_accepts_truthy_values() {
        unsafe {
            std::env::remove_var("NICTUI_BLE_DEBUG");
        }
        assert!(!ble_debug_enabled());

        unsafe {
            std::env::set_var("NICTUI_BLE_DEBUG", "1");
        }
        assert!(ble_debug_enabled());

        unsafe {
            std::env::set_var("NICTUI_BLE_DEBUG", "false");
        }
        assert!(!ble_debug_enabled());

        unsafe {
            std::env::remove_var("NICTUI_BLE_DEBUG");
        }
    }

    #[test]
    fn adapter_timeout_message_mentions_duration() {
        let message = ble_adapter_timeout_message(Duration::from_secs(7));
        assert!(message.contains("7s"));
    }

    #[test]
    fn adapter_acquisition_timeout_matches_platform_budget() {
        let timeout = adapter_acquisition_timeout();
        if cfg!(target_os = "macos") {
            assert_eq!(timeout, Duration::from_secs(25));
        } else {
            assert_eq!(timeout, Duration::from_secs(5));
        }
    }

    #[test]
    fn adapter_timeout_message_matches_platform_context() {
        let message = ble_adapter_timeout_message(Duration::from_secs(5));
        if cfg!(target_os = "macos") {
            assert!(message.contains("NSBluetoothAlwaysUsageDescription"));
            assert!(message.contains("Bluetooth/TCC permission"));
            assert!(message.contains("hosted wrapper"));
            assert!(message.contains("NicTUI.app"));
            assert!(message.contains("Finder"));
            assert!(message.contains("normal Terminal session"));
        } else {
            assert!(!message.contains("NSBluetoothAlwaysUsageDescription"));
        }
    }

    #[test]
    fn classifies_macos_tcc_timeouts_as_permission_blocked() {
        let report = classify_ble_operation_failure(
            "scan",
            anyhow!("{}", ble_adapter_timeout_message(Duration::from_secs(25))),
            None,
            None,
        );

        if cfg!(target_os = "macos") {
            assert_eq!(report.kind, BleReadinessKind::PermissionBlocked);
        } else {
            assert_eq!(report.kind, BleReadinessKind::ConnectivityFailed);
        }
    }

    #[test]
    fn classifies_missing_characteristics_as_protocol_mismatch() {
        assert_eq!(
            classify_ble_failure_kind(
                "TD-H3 BLE write characteristic 0000ff02-0000-1000-8000-00805f9b34fb not found",
                "connect"
            ),
            BleReadinessKind::ProtocolMismatch
        );
    }

    #[test]
    fn readiness_from_scan_reports_empty_discovery() {
        let report = assess_ble_readiness_from_devices(Vec::new(), None);
        assert_eq!(report.kind, BleReadinessKind::DiscoveryEmpty);
        assert!(!report.ok);
    }

    #[test]
    fn readiness_from_scan_reports_name_ambiguity() {
        let devices = vec![
            BleDevice {
                device_id: "1".to_string(),
                name: Some("TD-H3".to_string()),
                rssi: Some(-50),
            },
            BleDevice {
                device_id: "2".to_string(),
                name: Some("td h3".to_string()),
                rssi: Some(-55),
            },
        ];

        let report =
            assess_ble_readiness_from_devices(devices, Some(&BleTarget::Name("TD-H3".to_string())));
        assert_eq!(report.kind, BleReadinessKind::TargetAmbiguous);
        assert!(!report.ok);
    }
}
