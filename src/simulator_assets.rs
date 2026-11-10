//! Discovers user-owned simulator font assets without opening the source or ROM.

use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{Cursor, Read},
    path::PathBuf,
};

const MAX_CONFIG_BYTES: usize = 16 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const DEFAULT_CONFIG: &str = "Library/Application Support/NicFWSim/assets.plist";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetPaths {
    pub source: PathBuf,
    pub rom: PathBuf,
}

/// Finds simulator assets using the native NicFWSim configuration precedence.
///
/// Both `NICFW_SOURCE` and `NICFW_FONT_ROM` must be set together. Otherwise,
/// `NICFW_ASSET_CONFIG` selects a plist, defaulting on macOS to NicFWSim's
/// Application Support plist under `HOME`. Missing configuration returns `None`;
/// invalid configuration returns an error. Asset validation belongs to the
/// native font profile loader. Call only when starting a simulator session.
pub fn configured_paths() -> Result<Option<AssetPaths>> {
    configured_paths_with(
        std::env::var_os("NICFW_SOURCE"),
        std::env::var_os("NICFW_FONT_ROM"),
        std::env::var_os("NICFW_ASSET_CONFIG"),
        std::env::var_os("HOME"),
        cfg!(target_os = "macos"),
    )
}

fn configured_paths_with(
    source: Option<OsString>,
    rom: Option<OsString>,
    config: Option<OsString>,
    home: Option<OsString>,
    macos: bool,
) -> Result<Option<AssetPaths>> {
    match (source, rom) {
        (Some(source), Some(rom)) => {
            return Ok(Some(AssetPaths {
                source: checked_path(&source, "NICFW_SOURCE")?,
                rom: checked_path(&rom, "NICFW_FONT_ROM")?,
            }));
        }
        (None, None) => {}
        _ => bail!("Set both NICFW_SOURCE and NICFW_FONT_ROM, or leave both unset"),
    }

    let config = match (config, home) {
        (Some(config), _) => checked_path(&config, "NICFW_ASSET_CONFIG")?,
        (None, Some(home)) if macos => {
            let path = checked_path(&home, "HOME")?.join(DEFAULT_CONFIG);
            checked_path(path.as_os_str(), "Font asset configuration path")?
        }
        _ => return Ok(None),
    };
    let metadata = match fs::symlink_metadata(&config) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot inspect font asset configuration"),
    };
    ensure!(
        metadata.file_type().is_file() && metadata.len() <= MAX_CONFIG_BYTES as u64,
        "Font asset configuration must be a regular file of at most 16 KiB"
    );

    let mut options = OpenOptions::new();
    options.read(true);
    // Do not follow a swapped symlink or block on a swapped FIFO after inspection.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&config)
        .context("Cannot open font asset configuration")?;
    let metadata = file
        .metadata()
        .context("Cannot inspect opened font asset configuration")?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES as u64,
        "Font asset configuration must be a regular file of at most 16 KiB"
    );
    read_config(file).map(Some)
}

fn checked_path(path: &OsStr, name: &str) -> Result<PathBuf> {
    ensure!(
        !path.is_empty() && path.as_encoded_bytes().len() <= MAX_PATH_BYTES,
        "{name} must be a nonempty path of at most 4096 bytes"
    );
    ensure!(
        !path.to_string_lossy().chars().any(char::is_control),
        "{name} must not contain control characters"
    );
    Ok(PathBuf::from(path))
}

fn read_config(reader: impl Read) -> Result<AssetPaths> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("Cannot read font asset configuration")?;
    parse_config(&bytes)
}

fn parse_config(bytes: &[u8]) -> Result<AssetPaths> {
    ensure!(
        bytes.len() <= MAX_CONFIG_BYTES,
        "Font asset configuration exceeds 16 KiB"
    );
    let value =
        plist::Value::from_reader(Cursor::new(bytes)).context("Invalid font asset plist")?;
    let dictionary = value
        .as_dictionary()
        .context("Font asset plist must be a dictionary")?;
    let path = |key: &str| -> Result<PathBuf> {
        let value = dictionary
            .get(key)
            .and_then(plist::Value::as_string)
            .with_context(|| format!("Font asset plist requires a {key} string"))?;
        checked_path(OsStr::new(value), key)
    };
    Ok(AssetPaths {
        source: path("font_source")?,
        rom: path("font_rom")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/simulator-assets-tests")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, bytes: &[u8]) -> OsString {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            path.into_os_string()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn plist_bytes(source: plist::Value, rom: plist::Value, binary: bool) -> Vec<u8> {
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert("font_source".into(), source);
        dictionary.insert("font_rom".into(), rom);
        let value = plist::Value::Dictionary(dictionary);
        let mut bytes = Vec::new();
        if binary {
            value.to_writer_binary(&mut bytes).unwrap();
        } else {
            value.to_writer_xml(&mut bytes).unwrap();
        }
        bytes
    }

    fn assets(source: &str, rom: &str) -> AssetPaths {
        AssetPaths {
            source: source.into(),
            rom: rom.into(),
        }
    }

    #[test]
    fn missing_configuration_is_optional() {
        for macos in [false, true] {
            assert_eq!(
                configured_paths_with(None, None, None, None, macos).unwrap(),
                None
            );
            let fixture = Fixture::new();
            assert_eq!(
                configured_paths_with(
                    None,
                    None,
                    Some(fixture.0.join("missing.plist").into_os_string()),
                    Some(fixture.0.clone().into_os_string()),
                    macos,
                )
                .unwrap(),
                None
            );
        }
    }

    #[test]
    fn environment_pair_overrides_even_invalid_config_and_home() {
        assert_eq!(
            configured_paths_with(
                Some("/local/source".into()),
                Some("/local/font.bin".into()),
                Some("\ninvalid config".into()),
                Some("".into()),
                true,
            )
            .unwrap(),
            Some(assets("/local/source", "/local/font.bin"))
        );
    }

    #[test]
    fn environment_pair_must_be_complete_and_valid_without_fallback() {
        let fixture = Fixture::new();
        let config = fixture.write(
            "assets.plist",
            &plist_bytes("/source".into(), "/rom".into(), false),
        );
        for (source, rom) in [
            (Some("/source"), None),
            (None, Some("/rom")),
            (Some(""), Some("/rom")),
            (Some("/source"), Some("")),
            (Some("/source\n"), Some("/rom")),
            (Some("/source"), Some("/rom\0")),
        ] {
            assert!(
                configured_paths_with(
                    source.map(OsString::from),
                    rom.map(OsString::from),
                    Some(config.clone()),
                    None,
                    false,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn explicit_config_overrides_macos_default_even_when_missing_or_invalid() {
        let fixture = Fixture::new();
        fixture.write(
            DEFAULT_CONFIG,
            &plist_bytes("/default/source".into(), "/default/rom".into(), false),
        );
        let home = Some(fixture.0.clone().into_os_string());
        let config = fixture.write(
            "override.plist",
            &plist_bytes("/override/source".into(), "/override/rom".into(), true),
        );
        assert_eq!(
            configured_paths_with(None, None, Some(config), home.clone(), true).unwrap(),
            Some(assets("/override/source", "/override/rom"))
        );
        assert_eq!(
            configured_paths_with(
                None,
                None,
                Some(fixture.0.join("missing").into_os_string()),
                home.clone(),
                true,
            )
            .unwrap(),
            None
        );
        let malformed = fixture.write("malformed.plist", b"not a plist");
        assert!(configured_paths_with(None, None, Some(malformed), home, true).is_err());
    }

    #[test]
    fn default_config_is_macos_only() {
        let fixture = Fixture::new();
        fixture.write(
            DEFAULT_CONFIG,
            &plist_bytes("/source".into(), "/rom".into(), false),
        );
        let home = Some(fixture.0.clone().into_os_string());
        assert_eq!(
            configured_paths_with(None, None, None, home.clone(), true).unwrap(),
            Some(assets("/source", "/rom"))
        );
        assert_eq!(
            configured_paths_with(None, None, None, home, false).unwrap(),
            None
        );
    }

    #[test]
    fn xml_and_binary_plists_find_paths_without_reading_assets() {
        for binary in [false, true] {
            let bytes = plist_bytes(
                "source with spaces & symbols".into(),
                "font.bin".into(),
                binary,
            );
            assert_eq!(
                parse_config(&bytes).unwrap(),
                assets("source with spaces & symbols", "font.bin")
            );
            let fixture = Fixture::new();
            let config = fixture.write("assets.plist", &bytes);
            assert_eq!(
                configured_paths_with(None, None, Some(config), None, false).unwrap(),
                Some(assets("source with spaces & symbols", "font.bin"))
            );
        }
    }

    #[test]
    fn malformed_plists_and_wrong_field_types_are_errors() {
        for bytes in [
            b"".as_slice(),
            b"not a plist",
            b"bplist00",
            b"<plist><dict>",
        ] {
            assert!(parse_config(bytes).is_err());
        }
        for binary in [false, true] {
            for value in [
                plist::Value::Boolean(true),
                plist::Value::Array(Vec::new()),
                plist::Value::Dictionary(plist::Dictionary::new()),
            ] {
                assert!(parse_config(&plist_bytes(value.clone(), "/rom".into(), binary)).is_err());
                assert!(parse_config(&plist_bytes("/source".into(), value, binary)).is_err());
            }
        }
        assert!(parse_config(b"<plist version=\"1.0\"><array/></plist>").is_err());
        assert!(parse_config(b"<plist version=\"1.0\"><dict/></plist>").is_err());
        assert!(
            parse_config(
                b"<plist><dict><key>font_source</key><string>/source</string></dict></plist>"
            )
            .is_err()
        );
    }

    #[test]
    fn every_path_is_bounded_and_control_free() {
        let longest = "x".repeat(MAX_PATH_BYTES);
        assert!(checked_path(OsStr::new(&longest), "path").is_ok());
        for bad in [
            String::new(),
            "x".repeat(MAX_PATH_BYTES + 1),
            "/path\0suffix".into(),
            "/path\t".into(),
            "/path\r".into(),
            "/path\n".into(),
            "/path\u{7f}".into(),
            "/path\u{85}".into(),
        ] {
            assert!(checked_path(OsStr::new(&bad), "path").is_err());
            for binary in [false, true] {
                assert!(
                    parse_config(&plist_bytes(bad.clone().into(), "/rom".into(), binary)).is_err()
                );
                assert!(
                    parse_config(&plist_bytes("/source".into(), bad.clone().into(), binary))
                        .is_err()
                );
            }
            assert!(
                configured_paths_with(
                    Some(bad.clone().into()),
                    Some("/rom".into()),
                    None,
                    None,
                    false
                )
                .is_err()
            );
            assert!(
                configured_paths_with(
                    Some("/source".into()),
                    Some(bad.clone().into()),
                    None,
                    None,
                    false
                )
                .is_err()
            );
            assert!(
                configured_paths_with(None, None, Some(bad.clone().into()), None, false).is_err()
            );
            assert!(configured_paths_with(None, None, None, Some(bad.into()), true).is_err());
        }
    }

    #[test]
    fn config_size_limit_applies_to_parser_reader_and_file() {
        let mut bytes = plist_bytes("/source".into(), "/rom".into(), false);
        bytes.resize(MAX_CONFIG_BYTES, b' ');
        assert!(parse_config(&bytes).is_ok());
        assert!(read_config(Cursor::new(&bytes)).is_ok());
        let fixture = Fixture::new();
        let config = fixture.write("assets.plist", &bytes);
        assert!(configured_paths_with(None, None, Some(config.clone()), None, false).is_ok());
        bytes.push(b' ');
        assert!(parse_config(&bytes).is_err());
        assert!(read_config(Cursor::new(&bytes)).is_err());
        fixture.write("assets.plist", &bytes);
        assert!(configured_paths_with(None, None, Some(config.clone()), None, false).is_err());
        OpenOptions::new()
            .write(true)
            .open(config)
            .unwrap()
            .set_len(1024 * 1024 * 1024)
            .unwrap();
        assert!(
            configured_paths_with(
                None,
                None,
                Some(fixture.0.join("assets.plist").into_os_string()),
                None,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn reader_never_consumes_more_than_limit_plus_one() {
        let mut reader = std::io::repeat(b' ').take(1_000_000);
        assert!(read_config(&mut reader).is_err());
        assert_eq!(reader.limit(), 1_000_000 - MAX_CONFIG_BYTES as u64 - 1);
    }

    #[test]
    fn directories_and_symlink_configs_are_rejected() {
        let fixture = Fixture::new();
        assert!(
            configured_paths_with(
                None,
                None,
                Some(fixture.0.clone().into_os_string()),
                None,
                false
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            let target = fixture.write(
                "assets.plist",
                &plist_bytes("/source".into(), "/rom".into(), false),
            );
            let link = fixture.0.join("link.plist");
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert!(
                configured_paths_with(None, None, Some(link.into_os_string()), None, false)
                    .is_err()
            );
        }
    }
}
