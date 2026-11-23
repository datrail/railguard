//! What an MCP request body names: its method and tool.

use super::tree::{Kind, Member, Node};
use super::{Unparsed, json};
use crate::endpoint::{Resolution, resolve_endpoint};

/// What a body names as an MCP message. Each case but `Read` names nothing, for its own reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpReading {
    /// An empty body, or a JSON value: `method` and `tool` are absent where it doesn't name them.
    Read {
        method: Option<String>,
        tool: Option<String>,
    },
    /// Not read as a tree: not JSON, or past the budget.
    Unparsed(Unparsed),
    /// A JSON-RPC batch, which names one message per item.
    Batch,
    /// `method`, `params`, `params.name` or `params.arguments` repeated, letter case ignored, or
    /// beside a key that isn't JSON or holds a NUL, which a lenient reader may read as any; or
    /// `method`, or a `tools/call`'s `params.name`, a string that doesn't decode or holds a NUL.
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
            Err(why) => return Self::Unparsed(why),
        };
        if root.kind() == Kind::Array {
            return Self::Batch;
        }
        let method = root.get("method");
        if method.is_ambiguous() {
            return Self::Ambiguous;
        }
        let params = root.get("params");
        if params.is_ambiguous() {
            return Self::Ambiguous;
        }
        let name = params.get("name");
        if name.is_ambiguous() {
            return Self::Ambiguous;
        }
        if params.get("arguments").is_ambiguous() {
            return Self::Ambiguous;
        }
        let method = match read_text(method) {
            Ok(method) => method,
            Err(reading) => return reading,
        };
        let tool = match method.as_deref() {
            Some("tools/call") => match read_text(name) {
                Ok(tool) => tool,
                Err(reading) => return reading,
            },
            _ => None,
        };
        Self::Read { method, tool }
    }

    /// The JSON-RPC `method`, where it's a string.
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Read { method, .. } => method.as_deref(),
            Self::Unparsed(_) | Self::Batch | Self::Ambiguous => None,
        }
    }

    /// `params.name` on a `tools/call`, where it's a string.
    pub fn tool(&self) -> Option<&str> {
        match self {
            Self::Read { tool, .. } => tool.as_deref(),
            Self::Unparsed(_) | Self::Batch | Self::Ambiguous => None,
        }
    }
}

// A string's text, `None` for any other value; ambiguous for one that doesn't decode, or holds a
// NUL, which a reader cutting strings there reads short (`"tools/call\u0000"` as `tools/call`).
fn read_text(member: Member<'_>) -> Result<Option<String>, McpReading> {
    match member.found() {
        Some(node) if node.kind() == Kind::String => node
            .text()
            .filter(|text| !text.contains('\0'))
            .map(|text| Some(text.into_owned()))
            .ok_or(McpReading::Ambiguous),
        _ => Ok(None),
    }
}

/// What a message reading as `mcp`, sent to `path`, asks for. One without a method names no
/// endpoint this reader can key.
pub fn read_endpoint(path: &str, mcp: &McpReading) -> Resolution {
    resolve_endpoint(path, mcp.method().unwrap_or(""), mcp.tool())
}
