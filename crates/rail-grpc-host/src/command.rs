//! The command line both binaries take: `<role>` serves as that role, `probe [--ready]` asks
//! the server already serving.

use std::fmt;

/// What a server serves as; each binary carries the roles it lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// The command `arguments` give, the program's name not among them, or the line to print for a
/// usage error. `roles` are those this binary carries.
// The role has no default, so a deployment always says which one it runs.
pub fn parse_command(roles: &[Role], arguments: &[String]) -> Result<Command, String> {
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let role = match arguments.as_slice() {
        [name] => roles.iter().find(|role| role.as_str() == *name),
        _ => None,
    };
    match (arguments.as_slice(), role) {
        (_, Some(&role)) => Ok(Command::Serve(role)),
        (["probe"], None) => Ok(Command::Probe { ready: false }),
        (["probe", "--ready"], None) => Ok(Command::Probe { ready: true }),
        ([], None) => Err(format!("no role given; {}", format_usage(roles))),
        (_, None) => Err(format!(
            "unknown arguments {arguments:?}; {}",
            format_usage(roles)
        )),
    }
}

// `usage: rail proxy | rail probe [--ready]`, one alternative per role.
fn format_usage(roles: &[Role]) -> String {
    let program = crate::program_name();
    let mut alternatives: Vec<String> = roles
        .iter()
        .map(|role| format!("{program} {role}"))
        .collect();
    alternatives.push(format!("{program} probe [--ready]"));
    format!("usage: {}", alternatives.join(" | "))
}
