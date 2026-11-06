use rail_policy::{Endpoint, Resolution, ResolvedKey, resolve_endpoint};

const MOUNT: &str = "/mcp";

fn resolve(method: &str, tool_name: Option<&str>) -> Resolution {
    resolve_endpoint(MOUNT, method, tool_name)
}

fn get_resolved(text: &str) -> Resolution {
    Resolution::Judged(Endpoint::Resolved(text.parse().unwrap()))
}

// The gateway reads the control ones as unrecognised; here a tool name is text, made safe where
// it's logged.
#[test]
fn test_uses_every_part_of_the_key_verbatim() {
    for tool_name in [
        "Track_Package",
        "orders.v2",
        "track\npackage",
        "track\u{1b}package",
        "track\u{202e}package",
        "track\u{2028}package",
    ] {
        assert_eq!(
            resolve("tools/call", Some(tool_name)),
            get_resolved(&format!("/mcp#tools/call#{tool_name}")),
            "{tool_name:?}"
        );
    }
}

#[test]
fn test_forwards_a_session_or_discovery_message_unjudged() {
    let methods = [
        "initialize",
        "ping",
        "notifications/initialized",
        "notifications/cancelled",
        "tools/list",
        "prompts/list",
        "resources/list",
        "resources/templates/list",
    ];
    for method in methods {
        assert_eq!(resolve(method, None), Resolution::Discovery, "{method}");
    }
}

#[test]
fn test_reads_every_other_method_as_keyless() {
    for method in ["resources/read", "prompts/get", "completion/complete"] {
        assert_eq!(
            resolve(method, Some("track_package")),
            Resolution::Judged(Endpoint::Keyless),
            "{method}"
        );
    }
}

#[test]
fn test_reads_a_missing_method_as_unrecognised() {
    assert_eq!(
        resolve("", None),
        Resolution::Judged(Endpoint::Unrecognised)
    );
}

#[test]
fn test_reads_a_tools_call_naming_no_tool_as_unrecognised() {
    for tool_name in [None, Some("")] {
        assert_eq!(
            resolve("tools/call", tool_name),
            Resolution::Judged(Endpoint::Unrecognised),
            "{tool_name:?}"
        );
    }
}

#[test]
fn test_reads_a_key_past_rail_centers_cap_in_characters_as_unrecognised() {
    for c in ["t", "é"] {
        let fits = c.repeat(255 - "/mcp#tools/call#".len());
        assert_eq!(
            resolve("tools/call", Some(&fits)),
            get_resolved(&format!("/mcp#tools/call#{fits}")),
            "{c}"
        );
        assert_eq!(
            resolve("tools/call", Some(&format!("{fits}{c}"))),
            Resolution::Judged(Endpoint::Unrecognised),
            "{c}"
        );
    }
}

#[test]
fn test_reads_a_key_on_an_empty_path_as_unrecognised() {
    assert_eq!(
        resolve_endpoint("", "tools/call", Some("track_package")),
        Resolution::Judged(Endpoint::Unrecognised)
    );
}

#[test]
fn test_reads_a_resolved_key() {
    let key: ResolvedKey = "/mcp#tools/call#a#b".parse().unwrap();
    assert_eq!(
        (key.path(), key.method(), key.call()),
        ("/mcp", "tools/call", "a#b")
    );
    assert_eq!(key.as_str(), "/mcp#tools/call#a#b");

    for text in [
        "/mcp#tools/call",
        "/mcp#tools/call#",
        "#tools/call#a",
        "/mcp##a",
        "",
    ] {
        assert!(text.parse::<ResolvedKey>().is_err(), "{text:?}");
    }
}
