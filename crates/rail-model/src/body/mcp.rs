//! What an MCP request body names: its method and tool.

use std::borrow::Cow;

use super::tree::{Kind, Node};
use super::{Unparsed, json};
use crate::endpoint::{Resolution, resolve_endpoint};

/// What a body names as an MCP message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpReading {
    /// Not read as a tree (not JSON, or past the budget), a JSON-RPC batch, or with `method`,
    /// `params`, `params.name` or `params.arguments` repeated, letter case ignored, or beside a key
    /// that isn't JSON, which a lenient reader may read as any: what it names can't be told.
    Unreadable,
    /// An empty body, or a JSON value: `method` and `tool` are absent where it doesn't name them.
    Read {
        method: Option<String>,
        tool: Option<String>,
    },
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
        let Ok(root) = root else {
            return Self::Unreadable;
        };
        if root.kind() == Kind::Array {
            return Self::Unreadable;
        }
        let method = root.get("method");
        if method.is_ambiguous() {
            return Self::Unreadable;
        }
        let params = root.get("params");
        if params.is_ambiguous() {
            return Self::Unreadable;
        }
        let name = params.get("name");
        if name.is_ambiguous() {
            return Self::Unreadable;
        }
        if params.get("arguments").is_ambiguous() {
            return Self::Unreadable;
        }
        let method = method.text().map(Cow::into_owned);
        let tool = match method.as_deref() {
            Some("tools/call") => name.text().map(Cow::into_owned),
            _ => None,
        };
        Self::Read { method, tool }
    }

    /// The JSON-RPC `method`, where it's a string.
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Read { method, .. } => method.as_deref(),
            Self::Unreadable => None,
        }
    }

    /// `params.name` on a `tools/call`, where it's a string.
    pub fn tool(&self) -> Option<&str> {
        match self {
            Self::Read { tool, .. } => tool.as_deref(),
            Self::Unreadable => None,
        }
    }
}

/// What a message reading as `mcp`, sent to `path`, asks for. One without a method names no
/// endpoint this reader can key.
pub fn read_endpoint(path: &str, mcp: &McpReading) -> Resolution {
    resolve_endpoint(path, mcp.method().unwrap_or(""), mcp.tool())
}
