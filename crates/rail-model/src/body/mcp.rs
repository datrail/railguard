//! What an MCP request body names: its method and tool.

use super::tree::{Kind, Member, Node};
use super::{Unparsed, json};
use crate::endpoint::{Resolution, resolve_endpoint};

/// What a body names as an MCP message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpReading {
    /// What it names can't be told.
    Unreadable(Unreadable),
    /// An empty body, or a JSON value: `method` and `tool` are absent where it doesn't name them.
    Read {
        method: Option<String>,
        tool: Option<String>,
    },
}

/// Why what a body names as an MCP message can't be told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unreadable {
    /// Not read as a tree: not JSON, or past the budget.
    Unparsed(Unparsed),
    /// A JSON-RPC batch, which names one message per item.
    Batch,
    /// `method`, `params`, `params.name` or `params.arguments` repeated, letter case ignored, or
    /// beside a key that isn't JSON, which a lenient reader may read as any; or `method`, or a
    /// `tools/call`'s `params.name`, a string that doesn't decode.
    Ambiguous,
}

impl McpReading {
    /// Reads `body` as `RequestBody::mcp` does, with no budget, for a host that holds the bytes itself.
    pub fn read(body: &[u8]) -> Self {
        let tree = json::parse(body, &mut |_| true);
        let root = tree
            .as_ref()
            .map(|tree| tree.root(body))
            .map_err(|&why| why);
        Self::of(body, root)
    }

    /// `content`'s reading, `root` its JSON.
    pub(crate) fn of(content: &[u8], root: Result<Node<'_>, Unparsed>) -> Self {
        if content.is_empty() {
            return Self::Read {
                method: None,
                tool: None,
            };
        }
        let root = match root {
            Ok(root) => root,
            Err(why) => return Self::Unreadable(Unreadable::Unparsed(why)),
        };
        if root.kind() == Kind::Array {
            return Self::Unreadable(Unreadable::Batch);
        }
        let method = root.get("method");
        if method.is_ambiguous() {
            return Self::Unreadable(Unreadable::Ambiguous);
        }
        let params = root.get("params");
        if params.is_ambiguous() {
            return Self::Unreadable(Unreadable::Ambiguous);
        }
        let name = params.get("name");
        if name.is_ambiguous() {
            return Self::Unreadable(Unreadable::Ambiguous);
        }
        if params.get("arguments").is_ambiguous() {
            return Self::Unreadable(Unreadable::Ambiguous);
        }
        let method = match read_text(method) {
            Ok(method) => method,
            Err(why) => return Self::Unreadable(why),
        };
        let tool = match method.as_deref() {
            Some("tools/call") => match read_text(name) {
                Ok(tool) => tool,
                Err(why) => return Self::Unreadable(why),
            },
            _ => None,
        };
        Self::Read { method, tool }
    }

    /// The JSON-RPC `method`, where it's a string.
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Read { method, .. } => method.as_deref(),
            Self::Unreadable(_) => None,
        }
    }

    /// `params.name` on a `tools/call`, where it's a string.
    pub fn tool(&self) -> Option<&str> {
        match self {
            Self::Read { tool, .. } => tool.as_deref(),
            Self::Unreadable(_) => None,
        }
    }
}

// A string's text, `None` for any other value; ambiguous for one that doesn't decode, which
// readers differ on.
fn read_text(member: Member<'_>) -> Result<Option<String>, Unreadable> {
    match member.found() {
        Some(node) if node.kind() == Kind::String => node
            .text()
            .map(|text| Some(text.into_owned()))
            .ok_or(Unreadable::Ambiguous),
        _ => Ok(None),
    }
}

/// What a message reading as `mcp`, sent to `path`, asks for. One without a method names no
/// endpoint this reader can key.
pub fn read_endpoint(path: &str, mcp: &McpReading) -> Resolution {
    resolve_endpoint(path, mcp.method().unwrap_or(""), mcp.tool())
}
