use rail_policy::{Endpoint, Resolution, resolve_endpoint};

const MOUNT: &str = "/mcp";

fn resolve(method: &str, tool_name: Option<&str>) -> Resolution {
    resolve_endpoint(MOUNT, method, tool_name)
}

fn get_resolved(text: &str) -> Resolution {
    Resolution::Judged(Endpoint::Resolved(text.parse().unwrap()))
}

#[test]
fn test_uses_every_part_of_the_key_verbatim() {
    assert_eq!(
        resolve("tools/call", Some("Track_Package")),
        get_resolved("/mcp#tools/call#Track_Package")
    );
    assert_eq!(
        resolve("tools/call", Some("orders.v2")),
        get_resolved("/mcp#tools/call#orders.v2")
    );
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

// The gateway reads these as unrecognised; here a tool name is text, made safe where it's logged.
#[test]
fn test_resolves_a_tool_name_with_any_character() {
    for tool_name in [
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
fn test_reads_a_key_past_rail_centers_cap_as_unrecognised() {
    let fits = "t".repeat(255 - "/mcp#tools/call#".len());
    assert_eq!(
        resolve("tools/call", Some(&fits)),
        get_resolved(&format!("/mcp#tools/call#{fits}"))
    );
    assert_eq!(
        resolve("tools/call", Some(&format!("{fits}t"))),
        Resolution::Judged(Endpoint::Unrecognised)
    );
}

#[test]
fn test_counts_the_cap_in_characters() {
    let fits = "é".repeat(255 - "/mcp#tools/call#".len());
    assert!(matches!(
        resolve("tools/call", Some(&fits)),
        Resolution::Judged(Endpoint::Resolved(_))
    ));
}

#[test]
fn test_reads_a_key_on_an_empty_path_as_unrecognised() {
    assert_eq!(
        resolve_endpoint("", "tools/call", Some("track_package")),
        Resolution::Judged(Endpoint::Unrecognised)
    );
}
