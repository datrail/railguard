use rail_grpc_host::{Command, Role, parse_command};

const BOTH: [Role; 2] = [Role::Proxy, Role::Gateway];

// As the image runs it: the program by a path, named `rail`.
fn parse(roles: &[Role], arguments: &[&str]) -> Result<Command, String> {
    let command_line: Vec<String> = std::iter::once("/usr/local/bin/rail")
        .chain(arguments.iter().copied())
        .map(str::to_owned)
        .collect();
    parse_command(roles, &command_line)
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
    assert_eq!(
        parse(&BOTH, &[]),
        Err("no role given; usage: rail proxy | rail gateway | rail probe [--ready]".to_owned())
    );
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
        assert_eq!(
            parse(&[Role::Proxy], arguments),
            Err(format!(
                "unknown arguments {arguments:?}; usage: rail proxy | rail probe [--ready]"
            )),
        );
    }
}
