use std::io::Cursor;

fn profile() -> plist::Dictionary {
    plist::Value::from_reader(Cursor::new(include_bytes!("../macos/NicTUI.terminal")))
        .unwrap()
        .into_dictionary()
        .unwrap()
}

fn archive(data: &plist::Value) -> Vec<plist::Value> {
    plist::Value::from_reader(Cursor::new(data.as_data().unwrap()))
        .unwrap()
        .as_dictionary()
        .unwrap()["$objects"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn lcd_profile_is_opaque_and_uses_explicit_pixel_font_geometry() {
    let profile = profile();
    assert_eq!(profile["type"].as_string(), Some("Window Settings"));
    assert_eq!(profile["FontAntialias"].as_boolean(), Some(false));
    assert_eq!(profile["UseBoldFonts"].as_boolean(), Some(false));
    assert!(!profile.contains_key("CommandString"));
    let font = archive(&profile["Font"]);
    assert_eq!(font[2].as_string(), Some("AndaleMono"));
    let size = font[1].as_dictionary().unwrap()["NSSize"]
        .as_real()
        .unwrap();
    assert!((size - 11.022605).abs() < 0.000001);
    let width = size * (9.6015625 / 16.0) * profile["FontWidthSpacing"].as_real().unwrap();
    let height = size * 1.125 * profile["FontHeightSpacing"].as_real().unwrap();
    assert!((width - 6.0).abs() < 0.000001);
    assert!((height - 12.0).abs() < 0.000001);

    let color = archive(&profile["BackgroundColor"]);
    let bytes = color[1].as_dictionary().unwrap()["NSRGB"]
        .as_data()
        .unwrap();
    let components: Vec<f64> = std::str::from_utf8(bytes)
        .unwrap()
        .trim_end_matches('\0')
        .split_whitespace()
        .map(|component| component.parse().unwrap())
        .collect();
    assert_eq!(&components[..3], &[0.0, 0.0, 0.0]);
    assert!(components.len() == 3 || components.get(3) == Some(&1.0));
}

#[test]
fn lcd_profile_has_room_for_native_resolution_and_controls() {
    let profile = profile();
    assert!(profile["columnCount"].as_unsigned_integer().unwrap() >= 190);
    assert!(profile["rowCount"].as_unsigned_integer().unwrap() >= 80);
}

#[cfg(target_os = "macos")]
#[test]
fn gui_launcher_opens_a_private_profile_and_preserves_simulator_arguments() {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "nictui profile test {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fs::create_dir_all(root.join("App/Contents/MacOS")).unwrap();
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("App/Contents/Resources")).unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    fs::create_dir(root.join("sessions")).unwrap();
    let launcher = root.join("App/Contents/MacOS/nictui");
    fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/macos/launcher.sh"),
        &launcher,
    )
    .unwrap();
    fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/macos/NicTUI.terminal"),
        root.join("App/Contents/Resources/NicTUI.terminal"),
    )
    .unwrap();
    let fake_binary = root.join("App/Contents/MacOS/nictui-bin");
    fs::write(
        &fake_binary,
        "#!/bin/bash\nprintf '%s\\n' \"$@\" > \"$NICTUI_TEST_ARGS\"\n",
    )
    .unwrap();
    let fake_open = root.join("bin/open");
    fs::write(
        &fake_open,
        "#!/bin/bash\nprintf '%s\\n' \"$@\" > \"$NICTUI_TEST_OPEN\"\n",
    )
    .unwrap();
    for path in [&fake_binary, &fake_open] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let args = [
        "tui",
        "--simulate",
        "--simulator-endpoint",
        "127.0.0.1:12345",
        "--simulator-http-endpoint",
        "127.0.0.1:12346",
    ];
    let output = Command::new("/bin/bash")
        .arg(&launcher)
        .args(args)
        .env("TMPDIR", root.join("sessions"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env("NICTUI_TEST_OPEN", root.join("open.txt"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let opened = fs::read_to_string(root.join("open.txt")).unwrap();
    let opened: Vec<_> = opened.lines().collect();
    assert_eq!(&opened[..2], &["-a", "Terminal"]);
    let profile_path = std::path::Path::new(opened[2]);
    assert!(profile_path.starts_with(root.join("sessions")));
    assert_eq!(profile_path.extension().unwrap(), "terminal");
    let private_profile = plist::Value::from_file(profile_path).unwrap();
    let command = private_profile.as_dictionary().unwrap()["CommandString"]
        .as_string()
        .unwrap();
    assert!(command.starts_with("/bin/bash "));
    let command_file = profile_path.with_extension("command");
    let script = fs::read_to_string(&command_file).unwrap();
    assert!(script.contains("${0%.command}.terminal"));
    let output = Command::new("/bin/bash")
        .arg(&command_file)
        .env("NICTUI_TEST_ARGS", root.join("args.txt"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("args.txt"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        args
    );
    assert!(!command_file.exists());
    assert!(!profile_path.exists());
}
