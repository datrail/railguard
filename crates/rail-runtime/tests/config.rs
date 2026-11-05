use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use rail_runtime::{ConfigErrors, Described, EXIT_CODE, Entry, Level, Secret, Settings, Variables};

#[test]
fn text_is_trimmed_and_blank_is_unset() {
    let variables: Variables = [("A", " a \n"), ("B", " \t"), ("C", "")]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert_eq!(settings.get("A").text(), Some("a"));
    assert_eq!(settings.get("B").text(), None);
    assert_eq!(settings.get("C").text(), None);
    assert_eq!(settings.get("D").text(), None);
    assert!(settings.get("A").is_set());
    assert!(!settings.get("B").is_set());
    assert_eq!(settings.finish(), Ok(()));
}

#[test]
fn a_value_that_is_not_utf8_is_refused_once() {
    let variables: Variables = [(OsString::from("A"), OsString::from_vec(vec![0xff]))]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert!(settings.get("A").is_set());
    assert_eq!(settings.get("A").required(true).text(), None);
    assert_eq!(settings.get("A").secret(), None);

    assert_eq!(
        settings.finish(),
        Err(ConfigErrors(vec!["A is not valid UTF-8".to_owned()]))
    );
}

#[test]
fn parse_refusals_follow_the_name_and_are_all_kept() {
    let variables: Variables = [("N", "x"), ("M", "y")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    let number = |text: &str| {
        text.parse::<u16>()
            .map_err(|_| format!("is not a port, got: {text}"))
    };
    assert_eq!(settings.get("N").parsed(number), None);
    assert_eq!(settings.get("M").parsed(number), None);
    assert_eq!(settings.get("UNSET").parsed(number), None);
    let errors = settings.finish().unwrap_err();

    assert_eq!(
        errors.0,
        ["N is not a port, got: x", "M is not a port, got: y"]
    );
    assert_eq!(
        errors.to_string(),
        "Invalid configuration detected:\n  N is not a port, got: x\n  M is not a port, got: y"
    );
}

#[test]
fn a_boolean_is_true_or_false_case_folded() {
    let variables: Variables = [("T", "True"), ("F", "FALSE"), ("X", "1")]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert!(settings.get("T").boolean().unwrap_or(false));
    assert!(!settings.get("F").boolean().unwrap_or(true));
    assert!(settings.get("UNSET").boolean().unwrap_or(true));
    assert!(!settings.get("X").boolean().unwrap_or(false));
    assert_eq!(
        settings.finish().unwrap_err().0,
        ["X must be true or false, got: 1"]
    );
}

#[test]
fn a_refused_value_is_quoted_safely() {
    let variables: Variables = [("X", "a\u{1b}[31mb")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    settings.get("X").boolean().unwrap_or(false);

    assert_eq!(
        settings.finish().unwrap_err().0,
        ["X must be true or false, got: a\\u{1b}[31mb"]
    );
}

#[test]
fn a_secret_is_trimmed_and_never_debugs() {
    let variables: Variables = [("S", "hunter2\n")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    let secret = settings.get("S").secret().unwrap();

    assert_eq!(secret.expose(), "hunter2");
    assert_eq!(format!("{secret:?}"), "Secret(***)");
}

#[test]
#[ignore = "run by a_refused_configuration_exits_2"]
fn refused_configuration_child() {
    ConfigErrors(vec!["X must be true or false, got: 1".to_owned()]).exit();
}

#[test]
fn a_refused_configuration_exits_2() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "refused_configuration_child",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(EXIT_CODE));
    assert_eq!(EXIT_CODE, 2);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("Invalid configuration detected:\n  X must be true or false, got: 1\n"),
        "{stderr}"
    );
}

#[test]
fn a_required_parse_refuses_unset_and_what_it_rejects() {
    let variables: Variables = [("N", "x"), ("P", "8080")].into_iter().collect();
    let mut settings = Settings::new(&variables);
    let port = |text: &str| {
        text.parse::<u16>()
            .map_err(|_| format!("is not a port, got: {text}"))
    };

    assert_eq!(settings.get("P").required(true).parsed(port), Some(8080));
    assert_eq!(settings.get("N").required(true).parsed(port), None);
    assert_eq!(settings.get("UNSET").required(true).parsed(port), None);
    assert_eq!(
        settings.finish().unwrap_err().0,
        [
            "N is not a port, got: x",
            "UNSET is required and is unset or blank"
        ]
    );
}

#[test]
fn a_repeated_refusal_is_listed_once_and_counted_each_time() {
    let variables: Variables = [(OsString::from("A"), OsString::from_vec(vec![0xff]))]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    settings.get("A").text();
    let before = settings.refusals();
    settings.get("A").text();

    assert!(settings.refusals() > before);
    assert_eq!(settings.finish().unwrap_err().0, ["A is not valid UTF-8"]);
}

#[test]
fn a_value_past_its_length_in_characters_is_refused() {
    let variables: Variables = [("BYTES", "abcé"), ("PADDED", " abcd \n"), ("LONG", "abcdé")]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert_eq!(settings.get("BYTES").max_chars(4).text(), Some("abcé"));
    assert_eq!(settings.get("PADDED").max_chars(4).text(), Some("abcd"));
    assert_eq!(settings.get("LONG").max_chars(4).text(), None);
    assert_eq!(
        settings.get("UNSET").required(true).max_chars(4).text(),
        None
    );
    assert_eq!(
        settings.finish().unwrap_err().0,
        [
            "LONG has 5 characters, past the 4 allowed",
            "UNSET is required and is unset or blank"
        ]
    );
}

#[test]
fn the_reason_a_setting_is_required_is_said_only_when_it_is() {
    let variables = Variables::default();
    let mut settings = Settings::new(&variables);

    assert_eq!(
        settings
            .get("OFF")
            .required(false)
            .because("X is on")
            .text(),
        None
    );
    assert_eq!(
        settings.get("ON").required(true).because("X is on").text(),
        None
    );
    assert_eq!(
        settings.finish().unwrap_err().0,
        ["ON is required because X is on, and is unset or blank"]
    );
}

#[test]
fn a_refusal_and_a_retired_name_follow_the_name() {
    let variables: Variables = [("OLD", "1"), ("BLANK", " \n")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    settings.get("OLD").retired("NEW replaces it");
    settings.get("BLANK").retired("blank is unset");
    settings.get("GONE").retired("nothing reads it");
    settings.get("X").refuse("is wrong for a reason");

    assert_eq!(
        settings.finish().unwrap_err().0,
        [
            "OLD is no longer read; NEW replaces it",
            "X is wrong for a reason"
        ]
    );
}

#[test]
fn every_read_and_is_set_records_the_name_but_retired_and_refuse_do_not() {
    let variables: Variables = [("A", "a"), ("OLD", "1")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    settings.get("A").text();
    settings.get("B").parsed(|text| Ok(text.to_owned()));
    settings.get("C").boolean();
    settings.get("D").secret();
    settings.get("E").is_set();
    settings.get("OLD").retired("NEW replaces it");
    settings.get("GONE").retired("nothing reads it");
    settings.get("F").refuse("is wrong for a reason");

    assert_eq!(
        settings.names_read().iter().copied().collect::<Vec<_>>(),
        ["A", "B", "C", "D", "E"]
    );
}

#[test]
fn a_variable_set_under_the_prefix_and_never_read_is_found() {
    let variables: Variables = [
        ("RAIL_HOST_ID", "h"),
        ("RAIL_HOST_IDS", "typo"),
        ("RAIL_BLANK", " \n"),
        ("RAIL_OLD", "1"),
        ("RAIL_CHECKED", "1"),
        ("HOME", "/root"),
    ]
    .into_iter()
    .collect();
    let mut settings = Settings::new(&variables);

    settings.get("RAIL_HOST_ID").text();
    settings.get("RAIL_CHECKED").is_set();
    settings.get("RAIL_OLD").retired("nothing reads it");

    assert_eq!(settings.find_unread("RAIL_"), ["RAIL_HOST_IDS"]);
}

#[test]
fn a_name_that_is_not_utf8_is_found_lossily() {
    let mut name = b"RAIL_".to_vec();
    name.push(0xff);
    let variables: Variables = [(OsString::from_vec(name), OsString::from("x"))]
        .into_iter()
        .collect();

    assert_eq!(
        Settings::new(&variables).find_unread("RAIL_"),
        ["RAIL_\u{fffd}"]
    );
}

#[test]
fn a_described_value_is_kept_as_given_and_a_warning_says_what_it_weakens() {
    let mut described = Described::default();

    described.info("A", "a\u{1b}[31mb");
    described.warn("B", true, "it weakens this");

    assert_eq!(
        described.entries(),
        [
            Entry {
                name: "A",
                value: "a\u{1b}[31mb".to_owned(),
                level: Level::Info,
            },
            Entry {
                name: "B",
                value: "true".to_owned(),
                level: Level::Warn("it weakens this"),
            },
        ]
    );
    assert_eq!(
        described.names().into_iter().collect::<Vec<_>>(),
        ["A", "B"]
    );
}

#[test]
fn a_secret_is_described_only_as_set_or_unset() {
    let mut described = Described::default();

    described.secret("S", Some(&Secret::new("hunter2".to_owned())));
    described.secret("U", None);

    let values: Vec<_> = described
        .entries()
        .iter()
        .map(|entry| (entry.name, entry.value.as_str(), entry.level))
        .collect();
    assert_eq!(
        values,
        [("S", "set", Level::Info), ("U", "unset", Level::Info)]
    );
}

#[test]
fn a_name_read_and_set_but_not_described_is_described_as_read_and_a_secret_only_as_set() {
    let variables: Variables = [
        ("USED", "1"),
        ("UNUSED", " a\u{1b}[31mb\n"),
        ("TOKEN", "hunter2"),
        ("CHECKED", "yes"),
        ("BLANK", " "),
        ("OLD", "1"),
        ("NEVER_READ", "x"),
    ]
    .into_iter()
    .collect();
    let mut settings = Settings::new(&variables);
    settings.get("USED").text();
    settings.get("UNUSED").text();
    settings.get("TOKEN").secret();
    settings.get("CHECKED").is_set();
    settings.get("BLANK").text();
    settings.get("UNSET").text();
    settings.get("OLD").retired("nothing reads it");
    let mut described = Described::default();
    described.info("USED", "1 as used");

    settings.describe_rest(&mut described);

    let entries: Vec<_> = described
        .entries()
        .iter()
        .map(|entry| (entry.name, entry.value.as_str(), entry.level))
        .collect();
    assert_eq!(
        entries,
        [
            ("USED", "1 as used", Level::Info),
            ("CHECKED", "yes", Level::Info),
            ("TOKEN", "set", Level::Info),
            ("UNUSED", "a\u{1b}[31mb", Level::Info),
        ]
    );
}
