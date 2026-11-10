use clap::Parser;
use nictui::cli::{Cli, Commands, DemoView, Dispatch, dispatch};

#[test]
fn simulator_dispatch_has_no_physical_target() {
    for flag in ["--simulate", "--sim"] {
        let cli = Cli::try_parse_from(["nictui", "tui", flag, "--view", "remote"]).unwrap();
        match dispatch(cli).unwrap() {
            Dispatch::LaunchTui {
                port,
                demo,
                simulate,
                demo_view,
                simulator_http_endpoint,
            } => {
                assert_eq!(port.as_deref(), Some("sim://127.0.0.1:8788"));
                assert!(!demo);
                assert!(simulate);
                assert_eq!(demo_view, DemoView::Remote);
                assert!(simulator_http_endpoint.is_none());
            }
            Dispatch::Exit => panic!("simulation must launch the TUI"),
        }
    }
}

#[test]
fn simulator_rejects_demo_and_every_physical_target_before_dispatch() {
    assert!(Cli::try_parse_from(["nictui", "tui", "--simulate", "--demo"]).is_err());
    for flag in ["--port", "--ble-device", "--ble-name"] {
        assert!(Cli::try_parse_from(["nictui", "tui", "--simulate", flag, "never-open"]).is_err());
    }
}

#[test]
fn simulator_dispatch_rechecks_programmatically_injected_targets() {
    for target in [0, 1, 2] {
        let mut cli = Cli::try_parse_from(["nictui", "tui", "--simulate"]).unwrap();
        let Some(Commands::Tui(args)) = &mut cli.command else {
            unreachable!()
        };
        match target {
            0 => args.port.port = Some("never-open".into()),
            1 => args.port.ble_device = Some("never-open".into()),
            _ => args.port.ble_name = Some("never-scan".into()),
        }
        let Err(error) = dispatch(cli) else {
            panic!("physical target must be rejected")
        };
        assert!(error.to_string().contains("Simulation cannot be combined"));
    }
}

#[test]
fn explicit_sim_uri_is_labeled_and_validated_as_a_simulator() {
    let cli = Cli::try_parse_from(["nictui", "tui", "--port", "sim://127.0.0.1:8788"]).unwrap();
    assert!(matches!(
        dispatch(cli).unwrap(),
        Dispatch::LaunchTui { simulate: true, .. }
    ));
    let cli = Cli::try_parse_from(["nictui", "tui", "--port", "sim://192.0.2.1:8788"]).unwrap();
    assert!(dispatch(cli).is_err());
}

#[test]
fn simulator_http_endpoint_normalizes_numeric_loopback_addresses() {
    for (endpoint, normalized) in [
        ("127.0.0.1:12345", "http://127.0.0.1:12345"),
        ("http://127.0.0.1:12345", "http://127.0.0.1:12345"),
        ("http://127.0.0.1:12345/", "http://127.0.0.1:12345"),
        ("[::1]:12345", "http://[::1]:12345"),
        ("http://[::1]:12345", "http://[::1]:12345"),
        ("http://[::1]:12345/", "http://[::1]:12345"),
    ] {
        let cli = Cli::try_parse_from([
            "nictui",
            "tui",
            "--simulate",
            "--simulator-http-endpoint",
            endpoint,
        ])
        .unwrap();
        let Some(Commands::Tui(args)) = &cli.command else {
            unreachable!()
        };
        assert_eq!(args.simulator_http_endpoint.as_deref(), Some(normalized));
        match dispatch(cli).unwrap() {
            Dispatch::LaunchTui {
                simulate,
                simulator_http_endpoint,
                port,
                ..
            } => {
                assert!(simulate);
                assert_eq!(simulator_http_endpoint.as_deref(), Some(normalized));
                assert_eq!(port.as_deref(), Some("sim://127.0.0.1:8788"));
            }
            Dispatch::Exit => panic!("expected TUI dispatch"),
        }
    }
}

#[test]
fn simulator_http_endpoint_requires_explicit_simulate_and_preserves_target_conflicts() {
    for prefix in [
        vec!["nictui", "tui"],
        vec!["nictui", "tui", "--demo"],
        vec!["nictui", "tui", "--port", "sim://127.0.0.1:12345"],
        vec!["nictui", "tui", "--port", "/dev/must-not-open"],
        vec!["nictui", "tui", "--ble-device", "must-not-connect"],
        vec!["nictui", "tui", "--ble-name", "must-not-scan"],
    ] {
        let mut args = prefix;
        args.extend(["--simulator-http-endpoint", "127.0.0.1:12345"]);
        assert!(Cli::try_parse_from(args).is_err());
    }
    for flag in ["--port", "--ble-device", "--ble-name"] {
        assert!(
            Cli::try_parse_from([
                "nictui",
                "tui",
                "--simulate",
                flag,
                "never-open",
                "--simulator-http-endpoint",
                "127.0.0.1:12345",
            ])
            .is_err()
        );
    }
    assert!(
        Cli::try_parse_from([
            "nictui",
            "tui",
            "--simulate",
            "--demo",
            "--simulator-http-endpoint",
            "127.0.0.1:12345",
        ])
        .is_err()
    );
}

#[test]
fn simulator_http_endpoint_rejects_remote_names_schemes_paths_and_zero_ports() {
    for endpoint in [
        "localhost:12345",
        "http://localhost:12345",
        "192.0.2.1:12345",
        "http://192.0.2.1:12345",
        "0.0.0.0:12345",
        "[::]:12345",
        "127.0.0.1:0",
        "http://[::1]:0",
        "127.0.0.1",
        "127.0.0.1:65536",
        "https://127.0.0.1:12345",
        "sim://127.0.0.1:12345",
        "http://user@127.0.0.1:12345",
        "http://127.0.0.1:12345/api/state",
        "http://127.0.0.1:12345?query=1",
        "http://127.0.0.1:12345#fragment",
    ] {
        assert!(
            Cli::try_parse_from([
                "nictui",
                "tui",
                "--simulate",
                "--simulator-http-endpoint",
                endpoint,
            ])
            .is_err(),
            "unexpected endpoint accepted: {endpoint}"
        );
    }
}

#[test]
fn simulator_http_dispatch_revalidates_injected_values_before_any_target_resolution() {
    for arguments in [
        vec!["nictui", "tui"],
        vec!["nictui", "tui", "--demo"],
        vec!["nictui", "tui", "--port", "sim://127.0.0.1:12345"],
    ] {
        let mut cli = Cli::try_parse_from(arguments).unwrap();
        let Some(Commands::Tui(args)) = &mut cli.command else {
            unreachable!()
        };
        args.simulator_http_endpoint = Some("127.0.0.1:12345".into());
        let Err(error) = dispatch(cli) else {
            panic!("explicit simulation must be required")
        };
        assert!(error.to_string().contains("requires --simulate"));
    }
    for endpoint in ["127.0.0.1:12345", "http://192.0.2.1:12345"] {
        let mut cli = Cli::try_parse_from(["nictui", "tui", "--simulate"]).unwrap();
        let Some(Commands::Tui(args)) = &mut cli.command else {
            unreachable!()
        };
        args.simulator_http_endpoint = Some(endpoint.into());
        if endpoint.starts_with("http://192") {
            assert!(dispatch(cli).is_err());
        } else {
            assert!(matches!(dispatch(cli).unwrap(), Dispatch::LaunchTui {
                simulator_http_endpoint: Some(endpoint), ..
            } if endpoint == "http://127.0.0.1:12345"));
        }
    }
    let mut cli = Cli::try_parse_from(["nictui", "tui", "--simulate"]).unwrap();
    let Some(Commands::Tui(args)) = &mut cli.command else {
        unreachable!()
    };
    args.demo = true;
    args.simulator_http_endpoint = Some("127.0.0.1:12345".into());
    assert!(dispatch(cli).is_err());
}

#[test]
fn simulator_http_dispatch_is_passive_and_does_not_connect_to_either_endpoint() {
    let uart = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let uart_address = uart.local_addr().unwrap().to_string();
    let http_address = http.local_addr().unwrap().to_string();
    let cli = Cli::try_parse_from([
        "nictui",
        "tui",
        "--simulate",
        "--view",
        "remote",
        "--simulator-endpoint",
        &uart_address,
        "--simulator-http-endpoint",
        &http_address,
    ])
    .unwrap();
    assert!(matches!(dispatch(cli).unwrap(), Dispatch::LaunchTui {
        simulator_http_endpoint: Some(endpoint), ..
    } if endpoint == format!("http://{http_address}")));
    for listener in [&uart, &http] {
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn simulator_http_omission_preserves_existing_launch_modes() {
    for (arguments, expected_demo, expected_simulate) in [
        (vec!["nictui"], false, false),
        (vec!["nictui", "tui"], false, false),
        (vec!["nictui", "tui", "--demo"], true, false),
        (vec!["nictui", "tui", "--simulate"], false, true),
        (
            vec!["nictui", "tui", "--port", "sim://127.0.0.1:12345"],
            false,
            true,
        ),
    ] {
        match dispatch(Cli::try_parse_from(arguments).unwrap()).unwrap() {
            Dispatch::LaunchTui {
                demo,
                simulate,
                simulator_http_endpoint,
                ..
            } => {
                assert_eq!(demo, expected_demo);
                assert_eq!(simulate, expected_simulate);
                assert!(simulator_http_endpoint.is_none());
            }
            Dispatch::Exit => panic!("expected TUI dispatch"),
        }
    }
}
