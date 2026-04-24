---
name: "nictui-radio-cli"
description: "Use when a user wants an AI to inspect or modify a TD-H3 radio through the installed `nictui` command-line interface. This skill must use the NicTUI CLI only, requires NicSure mod firmware on the radio, prefers focused single-record updates over bulk writes, creates backups before changes, runs `--validate-only` before real writes, and verifies the result after changes."
---

# NicTUI Radio CLI

Use this skill when the user wants radio work done through `nictui` rather than the TUI or direct serial protocol code.

## Non-negotiable rules

- Use the `nictui` CLI only.
- Do not use the interactive TUI for radio changes.
- Do not patch EEPROM bytes directly or talk to the serial protocol unless the user explicitly asks for protocol debugging.
- The radio must be running NicSure mod firmware for live reads and writes.
- Some NicSure builds still reject live-mode handshakes. If live-mode commands repeatedly fail after recovery, stop treating block-level live access as available on that firmware.
- Prefer the smallest possible write:
  - `channels get/update/clear`
  - `settings get/set`
  - `bluetooth scan/connect/disconnect/status/on/off`
  - `groups get/set`
  - `scan-presets get/update`
  - `band-plan get/update`
  - `dtmf get/update`
  - `remote key/capture/probe`
- Always make a backup artifact before writing.
- Always run the matching `--validate-only` command before the real write.
- After a real write, re-read the changed item or section and confirm the exact result.
- Bulk writes are allowed only when the user explicitly asks for a bulk operation.
- Firmware flashing is never a casual step. Only do it when the user explicitly asks for firmware work.

## If `nictui` is missing

1. Check for the binary:
   ```bash
   command -v nictui
   ```
2. If it is missing, install it before doing anything else:
   ```bash
   curl -fsSL https://raw.githubusercontent.com/RCGV1/NicTUI/master/install.sh | bash
   ```
3. Re-check:
   ```bash
   command -v nictui
   nictui version
   ```
4. If installation fails, report the exact blocker and stop.

## Required startup sequence

1. Detect likely ports:
   ```bash
   nictui ports --verbose
   ```
   For BLE sessions:
   ```bash
   nictui bluetooth scan
   ```
2. Probe the radio. Prefer auto-detection first:
   ```bash
   nictui probe --json
   ```
   Or over BLE:
   ```bash
   nictui probe --ble-name TD-H3
   ```
3. If auto-detect is ambiguous or fails, inspect ports in JSON and probe a specific port:
   ```bash
   nictui ports --json
   export NICTUI_PORT="<serial-port-from-nictui-ports>"
   nictui probe --port "$NICTUI_PORT" --json
   ```
4. If the probe says `stock/original`, stop and tell the user to install NicSure firmware before any live read/write command.
5. If more than one BLE target matches a name, switch to `--ble-device` and use the UUID instead.
6. Resolve the target once and carry it into every command that talks to the radio:
   ```bash
   NICTUI_TARGET=(--port "$NICTUI_PORT")
   # or, for a resolved BLE device:
   export NICTUI_BLE_DEVICE="<uuid-from-nictui-bluetooth-scan>"
   NICTUI_TARGET=(--ble-device "$NICTUI_BLE_DEVICE")
   ```
7. Create the artifact directory:
   ```bash
   mkdir -p nictui-radio-session
   ```
8. Before the first write in a session, capture a read-only baseline:
   ```bash
   nictui doctor "${NICTUI_TARGET[@]}" --output-dir nictui-radio-session --json --codeplug
   ```

## Safe write workflow

1. Read the exact item that will change and save it under `nictui-radio-session/`.
2. Modify that saved JSON or CSV artifact.
3. Run the matching `--validate-only` write command.
4. Run the real write command.
5. Re-read the same item.
6. Compare the re-read result to the requested change.
7. Tell the user exactly what changed, what file was used, and how it was verified.

## Exact command patterns

Read one channel:
```bash
nictui channels get "${NICTUI_TARGET[@]}" --channel 25 --output nictui-radio-session/channel-25.json
```

Update one channel:
```bash
nictui channels update "${NICTUI_TARGET[@]}" --channel 25 --input nictui-radio-session/channel-25.json --validate-only
nictui channels update "${NICTUI_TARGET[@]}" --channel 25 --input nictui-radio-session/channel-25.json
nictui channels get "${NICTUI_TARGET[@]}" --channel 25 --output nictui-radio-session/channel-25-after.json
```

Clear one channel:
```bash
nictui channels clear "${NICTUI_TARGET[@]}" --channel 25 --validate-only
nictui channels clear "${NICTUI_TARGET[@]}" --channel 25
```

Read or set one setting:
```bash
nictui settings get "${NICTUI_TARGET[@]}" --setting 17 --output nictui-radio-session/setting-17.json
nictui settings set "${NICTUI_TARGET[@]}" --setting 17 --value 12 --validate-only
nictui settings set "${NICTUI_TARGET[@]}" --setting 17 --value 12
nictui settings get "${NICTUI_TARGET[@]}" --setting 17 --output nictui-radio-session/setting-17-after.json
```

Read or change Bluetooth:
```bash
nictui bluetooth status "${NICTUI_TARGET[@]}" --output nictui-radio-session/bluetooth.json
nictui bluetooth on "${NICTUI_TARGET[@]}" --validate-only
nictui bluetooth on "${NICTUI_TARGET[@]}"
nictui bluetooth status "${NICTUI_TARGET[@]}" --output nictui-radio-session/bluetooth-after.json
```

Scan or open BLE access:
```bash
nictui bluetooth scan
nictui bluetooth connect --name TD-H3
nictui probe --ble-name TD-H3
```

Read or update one group label:
```bash
nictui groups get "${NICTUI_TARGET[@]}" --group 3 --output nictui-radio-session/group-3.json
nictui groups set "${NICTUI_TARGET[@]}" --group 3 --label FRS --validate-only
nictui groups set "${NICTUI_TARGET[@]}" --group 3 --label FRS
nictui groups get "${NICTUI_TARGET[@]}" --group 3 --output nictui-radio-session/group-3-after.json
```

Read or update one scan preset:
```bash
nictui scan-presets get "${NICTUI_TARGET[@]}" --index 2 --output nictui-radio-session/scan-2.json
nictui scan-presets update "${NICTUI_TARGET[@]}" --index 2 --input nictui-radio-session/scan-2.json --validate-only
nictui scan-presets update "${NICTUI_TARGET[@]}" --index 2 --input nictui-radio-session/scan-2.json
nictui scan-presets get "${NICTUI_TARGET[@]}" --index 2 --output nictui-radio-session/scan-2-after.json
```

Read or update one band plan:
```bash
nictui band-plan get "${NICTUI_TARGET[@]}" --index 4 --output nictui-radio-session/band-4.json
nictui band-plan update "${NICTUI_TARGET[@]}" --index 4 --input nictui-radio-session/band-4.json --validate-only
nictui band-plan update "${NICTUI_TARGET[@]}" --index 4 --input nictui-radio-session/band-4.json
nictui band-plan get "${NICTUI_TARGET[@]}" --index 4 --output nictui-radio-session/band-4-after.json
```

Read or update one DTMF preset:
```bash
nictui dtmf get "${NICTUI_TARGET[@]}" --index 1 --output nictui-radio-session/dtmf-1.json
nictui dtmf update "${NICTUI_TARGET[@]}" --index 1 --input nictui-radio-session/dtmf-1.json --validate-only
nictui dtmf update "${NICTUI_TARGET[@]}" --index 1 --input nictui-radio-session/dtmf-1.json
nictui dtmf get "${NICTUI_TARGET[@]}" --index 1 --output nictui-radio-session/dtmf-1-after.json
```

Bulk section writes only when explicitly requested:
```bash
nictui settings read "${NICTUI_TARGET[@]}" --output nictui-radio-session/settings.json
nictui settings write "${NICTUI_TARGET[@]}" --input nictui-radio-session/settings.json --validate-only
nictui settings write "${NICTUI_TARGET[@]}" --input nictui-radio-session/settings.json
```

Full EEPROM backup and inspection:
```bash
nictui codeplug read "${NICTUI_TARGET[@]}" --output nictui-radio-session/radio.nfw
nictui codeplug inspect --input nictui-radio-session/radio.nfw --json
```

Firmware validation before a flash:
```bash
nictui firmware flash "${NICTUI_TARGET[@]}" --input firmware.bin --validate-only
```

Remote-control session:
```bash
nictui remote key "${NICTUI_TARGET[@]}" --key flashlight
nictui remote capture "${NICTUI_TARGET[@]}" --duration 8 --send menu --send up
nictui remote probe "${NICTUI_TARGET[@]}" --preset menu
nictui remote diagnose "${NICTUI_TARGET[@]}" --json
nictui remote probe "${NICTUI_TARGET[@]}" --bytes 0B,00
nictui remote pvojh-sweep "${NICTUI_TARGET[@]}" --gap-ms 0,50,100 --json
```

Remote guidance:
- `remote probe --preset ...` selects a logical action and lets NicTUI translate it to the programmer wire bytes. `remote probe --bytes ...` sends literal wire bytes unchanged.
- Start with `nictui remote diagnose "${NICTUI_TARGET[@]}" --json` when you need to know whether control is actually confirmed.
- `telemetry-primed` and `primed-telemetry-carrythrough` mean telemetry woke up, but control is still unconfirmed unless the report shows a decoded control delta and `remote_control_confirmed: true`.
- `nictui remote key` now prints reaction evidence. Do not describe RX, surfaced packets, or carrythrough telemetry as successful remote control unless a decoded delta was observed.

## BLE notes

- `nictui bluetooth on` only flips the radio-side Bluetooth setting. It does not connect your local session by itself.
- NicTUI can open the radio natively over BLE through `--ble-device`, `--ble-name`, `ble://<uuid>`, or `nictui bluetooth connect`.
- `nictui bluetooth connect` resolves and opens the BLE transport; it does not change the radio setting.
- The expected TD-H3 BLE programming transport uses:
  - service `0000ff00-0000-1000-8000-00805f9b34fb`
  - notify/read `0000ff01-0000-1000-8000-00805f9b34fb`
  - write `0000ff02-0000-1000-8000-00805f9b34fb`
- BLE programming and `remote` control are separate workflows. Use `remote` for key injection or packet capture, and use the other commands for codeplug and settings edits.

## What to report back

- The exact command path used.
- The exact record, index, or setting that changed.
- The artifact files written under `nictui-radio-session/`.
- The validation command that was run.
- The verification read that confirmed the change.

## When to stop instead of guessing

- `probe` cannot complete a handshake.
- The detected firmware is `stock/original`.
- More than one plausible port exists and the correct one is still unclear after `ports --json` and targeted probes.
- The requested change would require a bulk write when the user asked for a narrow change.
