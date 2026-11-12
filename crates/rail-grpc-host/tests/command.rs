use rail_grpc_host::{Command, Role, parse_command};

const BOTH: [Role; 2] = [Role::Proxy, Role::Gateway];

fn parse(roles: &[Role], arguments: &[&str]) -> Result<Command, String> {
    let arguments: Vec<String> = arguments
        .iter()
        .map(|&argument| argument.to_owned())
        .collect();
    parse_command(roles, &arguments)
}

#[test]
fn a_role_the_binary_carries_serves_as_it() {
    assert_eq!(parse(&BOTH, &["proxy"]), Ok(Command::Serve(Role::Proxy)));
    assert_eq!(
        parse(&BOTH, &["gateway"]),
        Ok(Command::Serve(Role::Gateway))
    );
}

#[test]
fn the_probe_names_no_role() {
    assert_eq!(
        parse(&[Role::Proxy], &["probe"]),
        Ok(Command::Probe { ready: false })
    );
    assert_eq!(
        parse(&[Role::Gateway], &["probe", "--ready"]),
        Ok(Command::Probe { ready: true })
    );
}

#[test]
fn no_role_is_a_usage_error_naming_the_roles_carried() {
    let error = parse(&BOTH, &[]).unwrap_err();

    assert!(error.starts_with("no role given; usage: "), "{error}");
    // The program's name is the test binary's here.
    let alternatives: Vec<&str> = error["no role given; usage: ".len()..]
        .split(" | ")
        .collect();
    assert_eq!(alternatives.len(), 3, "{error}");
    assert!(alternatives[0].ends_with(" proxy"), "{error}");
    assert!(alternatives[1].ends_with(" gateway"), "{error}");
    assert!(alternatives[2].ends_with(" probe [--ready]"), "{error}");
}

#[test]
fn anything_else_is_a_usage_error() {
    for arguments in [
        &["gateway"][..],
        &["proxy", "probe"],
        &["probe", "--live"],
        &["--ready"],
        &["Proxy"],
        &["proxy", "proxy"],
    ] {
        let error = parse(&[Role::Proxy], arguments).unwrap_err();

        assert!(
            error.starts_with(&format!("unknown arguments {arguments:?}; usage: ")),
            "{error}"
        );
        assert!(!error.contains("gateway |"), "{error}");
    }
}
