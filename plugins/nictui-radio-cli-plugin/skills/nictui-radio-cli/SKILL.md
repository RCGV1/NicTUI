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
- Prefer the smallest possible write:
  - `channels get/update/clear`
  - `settings get/set`
  - `scan-presets get/update`
  - `band-plan get/update`
  - `dtmf get/update`
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
2. Probe the radio. Prefer auto-detection first:
   ```bash
   nictui probe --json
   ```
3. If auto-detect is ambiguous or fails, inspect ports in JSON and probe a specific port:
   ```bash
   nictui ports --json
   nictui probe --port /dev/cu.usbserial-210 --json
   ```
4. If the probe says `stock/original`, stop and tell the user to install NicSure firmware before any live read/write command.
5. Create the artifact directory:
   ```bash
   mkdir -p .live-debug/ai-radio-session
   ```
6. Before the first write in a session, capture a read-only baseline:
   ```bash
   nictui doctor --output-dir .live-debug/ai-radio-session --json --codeplug
   ```

## Safe write workflow

1. Read the exact item that will change and save it under `.live-debug/ai-radio-session/`.
2. Modify that saved JSON or CSV artifact.
3. Run the matching `--validate-only` write command.
4. Run the real write command.
5. Re-read the same item.
6. Compare the re-read result to the requested change.
7. Tell the user exactly what changed, what file was used, and how it was verified.

## Exact command patterns

Read one channel:
```bash
nictui channels get --channel 25 --output .live-debug/ai-radio-session/channel-25.json
```

Update one channel:
```bash
nictui channels update --channel 25 --input .live-debug/ai-radio-session/channel-25.json --validate-only
nictui channels update --channel 25 --input .live-debug/ai-radio-session/channel-25.json
nictui channels get --channel 25 --output .live-debug/ai-radio-session/channel-25-after.json
```

Clear one channel:
```bash
nictui channels clear --channel 25 --validate-only
nictui channels clear --channel 25
```

Read or set one setting:
```bash
nictui settings get --setting 17 --output .live-debug/ai-radio-session/setting-17.json
nictui settings set --setting 17 --value 12 --validate-only
nictui settings set --setting 17 --value 12
nictui settings get --setting 17 --output .live-debug/ai-radio-session/setting-17-after.json
```

Read or update one scan preset:
```bash
nictui scan-presets get --index 2 --output .live-debug/ai-radio-session/scan-2.json
nictui scan-presets update --index 2 --input .live-debug/ai-radio-session/scan-2.json --validate-only
nictui scan-presets update --index 2 --input .live-debug/ai-radio-session/scan-2.json
nictui scan-presets get --index 2 --output .live-debug/ai-radio-session/scan-2-after.json
```

Read or update one band plan:
```bash
nictui band-plan get --index 4 --output .live-debug/ai-radio-session/band-4.json
nictui band-plan update --index 4 --input .live-debug/ai-radio-session/band-4.json --validate-only
nictui band-plan update --index 4 --input .live-debug/ai-radio-session/band-4.json
nictui band-plan get --index 4 --output .live-debug/ai-radio-session/band-4-after.json
```

Read or update one DTMF preset:
```bash
nictui dtmf get --index 1 --output .live-debug/ai-radio-session/dtmf-1.json
nictui dtmf update --index 1 --input .live-debug/ai-radio-session/dtmf-1.json --validate-only
nictui dtmf update --index 1 --input .live-debug/ai-radio-session/dtmf-1.json
nictui dtmf get --index 1 --output .live-debug/ai-radio-session/dtmf-1-after.json
```

Bulk section writes only when explicitly requested:
```bash
nictui settings read --output .live-debug/ai-radio-session/settings.json
nictui settings write --input .live-debug/ai-radio-session/settings.json --validate-only
nictui settings write --input .live-debug/ai-radio-session/settings.json
```

Full EEPROM backup and inspection:
```bash
nictui codeplug read --output .live-debug/ai-radio-session/radio.nfw
nictui codeplug inspect --input .live-debug/ai-radio-session/radio.nfw --json
```

Firmware validation before a flash:
```bash
nictui firmware flash --input firmware.bin --validate-only
```

## What to report back

- The exact command path used.
- The exact record, index, or setting that changed.
- The artifact files written under `.live-debug/ai-radio-session/`.
- The validation command that was run.
- The verification read that confirmed the change.

## When to stop instead of guessing

- `probe` cannot complete a handshake.
- The detected firmware is `stock/original`.
- More than one plausible port exists and the correct one is still unclear after `ports --json` and targeted probes.
- The requested change would require a bulk write when the user asked for a narrow change.
