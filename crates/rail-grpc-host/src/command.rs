//! The command line both binaries take: `<role>` serves as that role, `probe [--ready]` asks
//! the server already serving.

use std::fmt;
use std::path::Path;

use rail_policy::sanitize;

/// What a server serves as; each binary carries the roles it lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// On the agent's side: attaches its ticket to outgoing calls.
    Proxy,
    /// In front of an MCP server: judges each call against the bundle.
    Gateway,
}

impl Role {
    /// Its name on the command line and in the log.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Gateway => "gateway",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the command line asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Serve as this role.
    Serve(Role),
    /// Ask the server for liveness, or with `ready`, for readiness; the server answers for its
    /// own role, so the probe names none.
    Probe { ready: bool },
}

/// The command a command line gives, its program's name first, or the line to print for a usage
/// error. `roles` are those this binary carries.
// The role has no default, so a deployment always says which one it runs.
pub fn parse_command(roles: &[Role], command_line: &[String]) -> Result<Command, String> {
    let (program, arguments) = command_line
        .split_first()
        .map_or(("", &[][..]), |(program, arguments)| {
            (program.as_str(), arguments)
        });
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let usage = || format_usage(program, roles);
    let unknown = || format!("unknown arguments {arguments:?}; {}", usage());
    match arguments.as_slice() {
        ["probe"] => Ok(Command::Probe { ready: false }),
        ["probe", "--ready"] => Ok(Command::Probe { ready: true }),
        [] => Err(format!("no role given; {}", usage())),
        [name] => roles
            .iter()
            .find(|role| role.as_str() == *name)
            .map(|&role| Command::Serve(role))
            .ok_or_else(unknown),
        _ => Err(unknown()),
    }
}

// `usage: rail proxy | rail probe [--ready]`, one alternative per role; the program by the name
// it was run under, `rail` in its image.
fn format_usage(program: &str, roles: &[Role]) -> String {
    let name = Path::new(program).file_name().unwrap_or_default();
    let program = sanitize(&name.to_string_lossy()).to_string();
    let mut alternatives: Vec<String> = roles
        .iter()
        .map(|role| format!("{program} {role}"))
        .collect();
    alternatives.push(format!("{program} probe [--ready]"));
    format!("usage: {}", alternatives.join(" | "))
}
