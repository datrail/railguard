use rail_runtime::{
    CenterUrl, Level, RailCenterSettings, Settings, Variables, read_disabled, read_rail_center,
};
use rail_test_support::{describe, read_from};

use crate::common;

const TOKEN: &str = "rc_svc_abcdefghijklmnopqrstuvwxyzABCDEFGHIJK-_0123";

#[derive(Debug)]
enum Read {
    // Whatever was set, validated.
    Disabled(Option<RailCenterSettings>),
    Enabled(RailCenterSettings),
}

// The settings `pairs` describe, or every refusal.
fn read(pairs: &[(&str, &str)]) -> Result<Read, Vec<String>> {
    common::read(pairs, |settings| {
        let disabled = read_disabled(settings);
        let rail_center = read_rail_center(settings, disabled == Ok(false));
        Ok(if disabled? {
            Read::Disabled(rail_center?)
        } else {
            Read::Enabled(rail_center?.expect("required settings are read or refused"))
        })
    })
}

fn refusals(pairs: &[(&str, &str)]) -> Vec<String> {
    read(pairs).expect_err("refused")
}

fn refusal(pairs: &[(&str, &str)]) -> String {
    let refusals = refusals(pairs);
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    refusals.into_iter().next().unwrap()
}

fn enabled(pairs: &[(&str, &str)]) -> RailCenterSettings {
    match read(pairs) {
        Ok(Read::Enabled(settings)) => settings,
        other => panic!("{other:?}"),
    }
}

const URL: (&str, &str) = ("RAIL_CENTER_URL", "https://rc.example.com");
const BEARER: (&str, &str) = ("RAIL_AUTH_MODE", "bearer");

#[test]
fn rail_center_is_used_unless_disabled() {
    assert!(matches!(
        read(&[("RAIL_DISABLED", "TRUE")]),
        Ok(Read::Disabled(None))
    ));
    assert_eq!(
        refusal(&[]),
        "RAIL_CENTER_URL is required and is unset or blank"
    );
    assert_eq!(
        refusal(&[("RAIL_DISABLED", " false ")]),
        "RAIL_CENTER_URL is required and is unset or blank"
    );
    assert!(matches!(read(&[URL]), Ok(Read::Enabled(_))));
}

#[test]
fn disabled_reads_only_true_or_false_and_is_then_the_only_refusal() {
    for value in ["1", "yes", "on", "ture"] {
        assert_eq!(
            refusals(&[("RAIL_DISABLED", value)]),
            [format!("RAIL_DISABLED must be true or false, got: {value}")]
        );
    }
}

#[test]
fn a_blank_value_is_unset() {
    assert!(matches!(
        read(&[("RAIL_DISABLED", " \n"), URL]),
        Ok(Read::Enabled(_))
    ));
    assert_eq!(
        refusal(&[("RAIL_CENTER_URL", "  ")]),
        "RAIL_CENTER_URL is required and is unset or blank"
    );
    let settings = enabled(&[URL, ("RAIL_AUTH_MODE", ""), ("RAIL_AUTH_TOKEN", " ")]);
    assert!(settings.token.is_none());
}

#[test]
fn disabled_still_judges_what_is_set() {
    let read = read(&[
        ("RAIL_DISABLED", "true"),
        URL,
        BEARER,
        ("RAIL_AUTH_TOKEN", TOKEN),
    ]);
    assert!(matches!(read, Ok(Read::Disabled(Some(_)))), "{read:?}");

    let refused = refusal(&[("RAIL_DISABLED", "true"), ("RAIL_CENTER_URL", "ftp://rc")]);
    assert!(
        refused.starts_with("RAIL_CENTER_URL must start with"),
        "{refused}"
    );
    let refused = refusal(&[("RAIL_DISABLED", "true"), BEARER, ("RAIL_AUTH_TOKEN", "x")]);
    assert!(
        refused.starts_with("RAIL_AUTH_TOKEN doesn't start with rc_svc_"),
        "{refused}"
    );
}

#[test]
fn disabled_requires_nothing() {
    assert!(matches!(
        read(&[
            ("RAIL_DISABLED", "true"),
            BEARER,
            ("RAIL_CENTER_ALLOW_HTTP", "true")
        ]),
        Ok(Read::Disabled(None))
    ));
}

#[test]
fn retired_variables_are_refused_by_name_with_their_replacement() {
    let refused = refusals(&[
        URL,
        ("RAIL_PLUGIN_ENABLED", "true"),
        ("RAIL_PROXY_ALLOW_INSECURE_CREDENTIAL", "1"),
    ]);

    assert_eq!(
        refused,
        [
            "RAIL_PLUGIN_ENABLED is no longer read; Rail Center is used unless RAIL_DISABLED=true",
            "RAIL_PROXY_ALLOW_INSECURE_CREDENTIAL is no longer read; RAIL_CENTER_ALLOW_HTTP=true \
             allows an http:// RAIL_CENTER_URL",
        ]
    );
}

#[test]
fn a_retired_variable_carrying_no_value_is_not_refused() {
    assert!(
        read(&[
            URL,
            ("RAIL_PROXY_ALLOW_INSECURE_CREDENTIAL", ""),
            ("RAIL_PLUGIN_ENABLED", " ")
        ])
        .is_ok()
    );
}

#[test]
fn every_refusal_is_reported_at_once() {
    let refused = refusals(&[
        ("RAIL_PLUGIN_ENABLED", "false"),
        ("RAIL_CENTER_URL", "ftp://rc"),
        ("RAIL_CENTER_ALLOW_HTTP", "maybe"),
        ("RAIL_AUTH_MODE", "bearer"),
    ]);

    assert_eq!(refused.len(), 4, "{refused:?}");
}

#[test]
fn an_http_refusal_does_not_hide_the_tokens() {
    let refused = refusals(&[
        ("RAIL_CENTER_URL", "http://rc"),
        BEARER,
        ("RAIL_AUTH_TOKEN", "not-a-token"),
    ]);

    assert_eq!(refused.len(), 2, "{refused:?}");
    assert!(
        refused[0].starts_with("RAIL_CENTER_URL is http://"),
        "{refused:?}"
    );
    assert!(refused[1].starts_with("RAIL_AUTH_TOKEN "), "{refused:?}");
}

#[test]
fn http_needs_allow_http() {
    let refused = refusal(&[("RAIL_CENTER_URL", "http://rc.example.com")]);
    assert!(
        refused.starts_with("RAIL_CENTER_URL is http://"),
        "{refused}"
    );

    // Loopback too: the setting says what's allowed, not where.
    assert!(read(&[("RAIL_CENTER_URL", "http://127.0.0.1:8000")]).is_err());

    let settings = enabled(&[
        ("RAIL_CENTER_URL", "http://rc:8000"),
        ("RAIL_CENTER_ALLOW_HTTP", "true"),
    ]);
    assert!(!settings.url.is_https());
}

#[test]
fn the_auth_mode_is_none_unless_bearer() {
    assert!(enabled(&[URL]).token.is_none());
    assert!(
        enabled(&[URL, ("RAIL_AUTH_MODE", " NONE ")])
            .token
            .is_none()
    );

    let settings = enabled(&[
        URL,
        ("RAIL_AUTH_MODE", "Bearer"),
        ("RAIL_AUTH_TOKEN", TOKEN),
    ]);
    assert_eq!(settings.token.unwrap().expose(), TOKEN);
}

#[test]
fn a_token_written_to_a_file_keeps_its_meaning() {
    let padded = format!("{TOKEN}\n");

    let settings = enabled(&[URL, BEARER, ("RAIL_AUTH_TOKEN", &padded)]);

    assert_eq!(settings.token.unwrap().expose(), TOKEN);
}

#[test]
fn an_unknown_mode_names_what_is_accepted_without_quoting_it() {
    assert_eq!(
        refusal(&[URL, ("RAIL_AUTH_MODE", TOKEN)]),
        "RAIL_AUTH_MODE must be none or bearer, and is something else (50 characters)"
    );
}

#[test]
fn bearer_without_a_token_is_refused() {
    for token in [None, Some(""), Some(" \n")] {
        let mut pairs = vec![URL, BEARER];
        pairs.extend(token.map(|token| ("RAIL_AUTH_TOKEN", token)));
        assert_eq!(
            refusal(&pairs),
            "RAIL_AUTH_TOKEN is required because RAIL_AUTH_MODE is bearer, and is unset or blank"
        );
    }
}

#[test]
fn a_token_beside_none_is_refused() {
    assert!(refusal(&[URL, ("RAIL_AUTH_TOKEN", TOKEN)]).starts_with(
        "RAIL_AUTH_MODE is unset, which is none and sends no credential, but RAIL_AUTH_TOKEN is set"
    ));
    assert!(
        refusal(&[URL, ("RAIL_AUTH_MODE", "none"), ("RAIL_AUTH_TOKEN", TOKEN)])
            .starts_with("RAIL_AUTH_MODE is none and sends no credential")
    );
}

#[test]
fn only_a_rail_center_service_token_is_read() {
    let secret = &TOKEN["rc_svc_".len()..];
    let cases = [
        (format!("rc_pat_{secret}"), "doesn't start with rc_svc_"),
        (secret.to_owned(), "doesn't start with rc_svc_"),
        (
            format!("rc_svc_{}", &secret[1..]),
            "has 42 characters after rc_svc_",
        ),
        (format!("{TOKEN}A"), "has 44 characters after rc_svc_"),
        (
            format!("rc_svc_{}+{}", &secret[..10], &secret[11..]),
            "at offset 17",
        ),
        (
            format!("rc_svc_{}\n{}", &secret[..10], &secret[11..]),
            "at offset 17",
        ),
        (
            format!("rc_svc_{}é{}", &secret[..10], &secret[12..]),
            "at offset 17",
        ),
    ];

    for (token, reason) in cases {
        let refused = refusal(&[URL, BEARER, ("RAIL_AUTH_TOKEN", &token)]);
        assert!(refused.starts_with("RAIL_AUTH_TOKEN "), "{refused}");
        assert!(refused.contains(reason), "{token:?}: {refused}");
        assert!(!refused.contains(&secret[..8]), "{refused}");
    }
}

#[test]
fn a_secret_never_debugs() {
    let settings = enabled(&[URL, BEARER, ("RAIL_AUTH_TOKEN", TOKEN)]);

    assert!(!format!("{settings:?}").contains(&TOKEN[8..]));
}

#[test]
fn a_url_that_cannot_carry_a_route_is_refused() {
    let cases = [
        ("rail-center:8000", "must start with http:// or https://"),
        ("//rail-center:8000", "must start with http:// or https://"),
        ("ftp://rail-center", "must start with http:// or https://"),
        ("http:rail-center", "must start with http:// or https://"),
        ("https://", "names no host"),
        ("https:///rc", "names no host"),
        (
            "https://rc.example.com?tenant=a",
            "must carry no query or fragment",
        ),
        (
            "https://rc.example.com/#frag",
            "must carry no query or fragment",
        ),
        (
            "https://rc.example.com:port",
            "is not a URL: invalid port number",
        ),
        ("https://[::1", "is not a URL"),
        ("https://rc.example.com\\@evil", "a backslash"),
        ("https://rc example.com", "at offset 10"),
        ("https://rc.exämple.com", "at offset 13"),
        ("https://%72c.example.com", "percent-encoded host"),
        ("http://0x7f.1", "reads as another"),
        ("http://[0:0::1]:80", "reads as another"),
    ];

    for (url, reason) in cases {
        let refused = CenterUrl::parse(url).expect_err(url);
        assert!(refused.contains(reason), "{url}: {refused}");
    }
}

#[test]
fn a_credential_in_the_url_is_refused_without_quoting_it() {
    for url in [
        "https://svc:s3cret@rc.example.com",
        "https://s3cret@rc.example.com",
        "https://:s3cret@rc.example.com",
        "https://svc:s3cret@rc.example.com?tenant=a",
    ] {
        let refused = refusal(&[("RAIL_CENTER_URL", url)]);
        assert!(
            refused.starts_with("RAIL_CENTER_URL carries a credential"),
            "{refused}"
        );
        assert!(!refused.contains("s3cret"));
    }
    // Before the parser: its own message could quote the authority.
    let refused = refusal(&[(
        "RAIL_CENTER_URL",
        "https://svc:hunter\u{ff20}2@rc.example.com",
    )]);
    assert!(!refused.contains("hunter"), "{refused}");
}

#[test]
fn the_route_survives_whatever_the_base_ends_in() {
    for base in [
        "https://rc.example.com",
        "https://rc.example.com/",
        "https://rc.example.com#",
        "https://rc.example.com?",
        "HTTPS://RC.example.com//",
    ] {
        let url = CenterUrl::parse(base).unwrap();
        assert_eq!(
            url.endpoint("/v1/tickets", &[("host_id", "h")]).as_str(),
            "https://rc.example.com/v1/tickets?host_id=h",
            "{base}"
        );
        assert_eq!(url.to_string(), "https://rc.example.com");
    }
}

#[test]
fn the_base_keeps_its_port_and_its_path_prefix() {
    let url = CenterUrl::parse("http://127.0.0.1:8000/rc/").unwrap();

    assert_eq!(
        url.endpoint("/v1/policy-bundle", &[]).as_str(),
        "http://127.0.0.1:8000/rc/v1/policy-bundle"
    );
    assert_eq!(url.to_string(), "http://127.0.0.1:8000/rc");
}

#[test]
fn query_values_are_encoded() {
    let url = CenterUrl::parse("https://rc.example.com").unwrap();

    let endpoint = url.endpoint("/v1/tickets", &[("sandbox_name", "a&b=c #d")]);

    assert_eq!(endpoint.query(), Some("sandbox_name=a%26b%3Dc+%23d"));
}

#[test]
fn an_unreadable_mode_is_the_only_refusal() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let variables: Variables = [
        (
            OsString::from("RAIL_CENTER_URL"),
            OsString::from("https://rc.example.com"),
        ),
        (
            OsString::from("RAIL_AUTH_MODE"),
            OsString::from_vec(vec![0xff]),
        ),
        (OsString::from("RAIL_AUTH_TOKEN"), OsString::from(TOKEN)),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        read_from(&variables, |settings| read_rail_center(settings, true)).unwrap_err(),
        ["RAIL_AUTH_MODE is not valid UTF-8"]
    );
}

#[test]
fn the_settings_describe_themselves_with_their_defaults() {
    assert_eq!(
        describe(&[URL], |settings| read_rail_center(settings, true)),
        [
            (
                "RAIL_CENTER_URL",
                "https://rc.example.com".to_owned(),
                Level::Info
            ),
            ("RAIL_CENTER_ALLOW_HTTP", "false".to_owned(), Level::Info),
            ("RAIL_AUTH_MODE", "none".to_owned(), Level::Info),
            ("RAIL_AUTH_TOKEN", "unset".to_owned(), Level::Info),
        ]
    );
}

#[test]
fn the_token_is_described_only_as_set() {
    let described = describe(&[URL, BEARER, ("RAIL_AUTH_TOKEN", TOKEN)], |settings| {
        read_rail_center(settings, true)
    });

    assert_eq!(
        described[2..],
        [
            ("RAIL_AUTH_MODE", "bearer".to_owned(), Level::Info),
            ("RAIL_AUTH_TOKEN", "set".to_owned(), Level::Info),
        ]
    );
    let secret = TOKEN.strip_prefix("rc_svc_").unwrap();
    for (name, value, _) in &described {
        assert!(!value.contains(secret), "{name} shows the token");
    }
}

#[test]
fn an_http_url_is_described_as_a_warning() {
    let described = describe(
        &[
            ("RAIL_CENTER_URL", "http://rc.internal:8080/"),
            ("RAIL_CENTER_ALLOW_HTTP", "true"),
        ],
        |settings| read_rail_center(settings, true),
    );

    assert_eq!(
        described[..2],
        [
            (
                "RAIL_CENTER_URL",
                "http://rc.internal:8080".to_owned(),
                Level::Warn("the credential and what Rail Center returns cross in the clear")
            ),
            ("RAIL_CENTER_ALLOW_HTTP", "true".to_owned(), Level::Info),
        ]
    );
}

#[test]
fn disabled_is_described_as_a_warning() {
    assert_eq!(
        describe(&[], read_disabled),
        [("RAIL_DISABLED", "false".to_owned(), Level::Info)]
    );
    assert_eq!(
        describe(&[("RAIL_DISABLED", "TRUE")], read_disabled),
        [(
            "RAIL_DISABLED",
            "true".to_owned(),
            Level::Warn("Rail Center isn't used: nothing is fetched from it")
        )]
    );
}

#[test]
fn anything_refused_is_an_err() {
    let cases: [(&[(&str, &str)], bool); 5] = [
        (&[("RAIL_CENTER_URL", "http://rc.example.com")], true),
        (&[("RAIL_CENTER_URL", "http://rc.example.com")], false),
        (&[URL, ("RAIL_PLUGIN_ENABLED", "true")], true),
        (&[URL, BEARER, ("RAIL_AUTH_TOKEN", "x")], false),
        (
            &[
                ("RAIL_DISABLED", "true"),
                ("RAIL_CENTER_URL", "http://rc.example.com"),
            ],
            false,
        ),
    ];
    for (pairs, required) in cases {
        let variables: Variables = pairs.iter().copied().collect();
        let mut settings = Settings::new(&variables);

        assert!(
            read_rail_center(&mut settings, required).is_err(),
            "{pairs:?}"
        );
    }
}
