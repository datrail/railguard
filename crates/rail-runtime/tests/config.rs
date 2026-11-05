use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use rail_runtime::{
    ConfigErrors, EXIT_CODE, Entry, Level, Refused, Secret, Settings, Source, Startup, Variables,
};

// What `settings` logs, which must refuse nothing.
fn finish(settings: Settings) -> Startup {
    settings.finish("RAIL_", Some(())).unwrap().1
}

fn errors(settings: Settings) -> Vec<String> {
    settings.finish("RAIL_", None::<()>).unwrap_err().0
}

fn entry(name: &'static str, value: &str, level: Level, source: Source) -> Entry {
    Entry {
        name,
        value: value.to_owned(),
        level,
        source,
    }
}

#[test]
fn text_is_trimmed_and_blank_is_unset() {
    let variables: Variables = [("A", " a \n"), ("B", " \t"), ("C", "")]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert_eq!(settings.get("A").text(), Ok(Some("a")));
    assert_eq!(settings.get("B").text(), Ok(None));
    assert_eq!(settings.get("C").text(), Ok(None));
    assert_eq!(settings.get("D").text(), Ok(None));
    assert!(settings.get("A").is_set());
    assert!(!settings.get("B").is_set());
    assert!(settings.finish("RAIL_", Some(())).is_ok());
}

#[test]
fn a_value_that_is_not_utf8_is_refused_once() {
    let variables: Variables = [(OsString::from("A"), OsString::from_vec(vec![0xff]))]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert!(settings.get("A").is_set());
    assert_eq!(
        settings.get("A").required_if(true, "X is on").text(),
        Err(Refused)
    );
    assert_eq!(settings.get("A").secret(), Err(Refused));

    assert_eq!(
        settings.finish("RAIL_", None::<()>).unwrap_err(),
        ConfigErrors(vec!["A is not valid UTF-8".to_owned()])
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
    assert_eq!(settings.get("N").parsed(number), Err(Refused));
    assert_eq!(settings.get("M").parsed(number), Err(Refused));
    assert_eq!(settings.get("UNSET").parsed(number), Ok(None));
    let errors = settings.finish("RAIL_", None::<()>).unwrap_err();

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

    assert_eq!(settings.get("T").boolean(), Ok(Some(true)));
    assert_eq!(settings.get("F").boolean(), Ok(Some(false)));
    assert_eq!(settings.get("UNSET").boolean(), Ok(None));
    assert_eq!(settings.get("X").boolean(), Err(Refused));
    assert_eq!(errors(settings), ["X must be true or false, got: 1"]);
}

#[test]
fn a_refused_value_is_quoted_safely() {
    let variables: Variables = [("X", "a\u{1b}[31mb")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    let _ = settings.get("X").boolean();

    assert_eq!(
        errors(settings),
        ["X must be true or false, got: a\\u{1b}[31mb"]
    );
}

#[test]
fn a_secret_is_trimmed_and_never_debugs() {
    let variables: Variables = [("S", "hunter2\n")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    let secret = settings.get("S").secret().unwrap().unwrap();

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

    assert_eq!(output.status.code(), Some(EXIT_CODE.into()));
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

    assert_eq!(
        settings.get("P").required_if(true, "X is on").parsed(port),
        Ok(Some(8080))
    );
    assert_eq!(
        settings.get("N").required_if(true, "X is on").parsed(port),
        Err(Refused)
    );
    assert_eq!(
        settings
            .get("UNSET")
            .required_if(true, "X is on")
            .parsed(port),
        Err(Refused)
    );
    assert_eq!(
        errors(settings),
        [
            "N is not a port, got: x",
            "UNSET is required because X is on, and is unset or blank"
        ]
    );
}

#[test]
fn a_repeated_refusal_is_listed_once_and_refused_each_time() {
    let variables: Variables = [(OsString::from("A"), OsString::from_vec(vec![0xff]))]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert_eq!(settings.get("A").text(), Err(Refused));
    assert_eq!(settings.get("A").text(), Err(Refused));

    assert_eq!(errors(settings), ["A is not valid UTF-8"]);
}

#[test]
fn a_value_past_its_length_in_characters_is_refused() {
    let variables: Variables = [("BYTES", "abcé"), ("PADDED", " abcd \n"), ("LONG", "abcdé")]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert_eq!(settings.get("BYTES").max_chars(4).text(), Ok(Some("abcé")));
    assert_eq!(settings.get("PADDED").max_chars(4).text(), Ok(Some("abcd")));
    assert_eq!(settings.get("LONG").max_chars(4).text(), Err(Refused));
    assert_eq!(
        settings
            .get("UNSET")
            .required_if(true, "X is on")
            .max_chars(4)
            .text(),
        Err(Refused)
    );
    assert_eq!(
        errors(settings),
        [
            "LONG has 5 characters, past the 4 allowed",
            "UNSET is required because X is on, and is unset or blank"
        ]
    );
}

#[test]
fn the_reason_a_setting_is_required_is_said_only_when_it_is() {
    let variables = Variables::default();
    let mut settings = Settings::new(&variables);

    assert_eq!(
        settings.get("OFF").required_if(false, "X is on").text(),
        Ok(None)
    );
    assert_eq!(
        settings.get("ON").required_if(true, "X is on").text(),
        Err(Refused)
    );
    assert_eq!(
        errors(settings),
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
    assert_eq!(settings.get("X").refuse("is wrong for a reason"), Refused);

    assert_eq!(
        errors(settings),
        [
            "OLD is no longer read; NEW replaces it",
            "X is wrong for a reason"
        ]
    );
}

#[test]
fn every_name_read_or_checked_has_an_entry_and_a_retired_one_has_none() {
    let variables: Variables = [("A", "a"), ("OLD", "")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    let _ = settings.get("A").text();
    let _ = settings.get("B").parsed(|text| Ok(text.to_owned()));
    let _ = settings.get("C").boolean();
    let _ = settings.get("D").secret();
    settings.get("E").is_set();
    settings.get("OLD").retired("NEW replaces it");
    settings.get("GONE").retired("nothing reads it");

    let names: Vec<_> = finish(settings)
        .entries
        .iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["A", "B", "C", "D", "E"]);
}

#[test]
fn a_variable_set_under_the_prefix_and_never_read_is_found() {
    let variables: Variables = [
        ("RAIL_HOST_ID", "h"),
        ("RAIL_HOST_IDS", "typo"),
        ("RAIL_BLANK", " \n"),
        ("RAIL_CHECKED", "1"),
        ("RAIL_DESCRIBED", "1"),
        ("HOME", "/root"),
    ]
    .into_iter()
    .collect();
    let mut settings = Settings::new(&variables);

    let _ = settings.get("RAIL_HOST_ID").text();
    settings.get("RAIL_CHECKED").is_set();
    settings.describe("RAIL_DESCRIBED", 1);

    assert_eq!(finish(settings).unread, ["RAIL_HOST_IDS"]);
}

#[test]
fn a_retired_name_carrying_no_value_is_not_unread() {
    let variables: Variables = [("RAIL_OLD", " "), ("RAIL_TYPO", "1")]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    settings.get("RAIL_OLD").retired("nothing reads it");

    assert_eq!(finish(settings).unread, ["RAIL_TYPO"]);
}

#[test]
fn a_name_that_is_not_utf8_is_found_lossily() {
    let mut name = b"RAIL_".to_vec();
    name.push(0xff);
    let variables: Variables = [(OsString::from_vec(name), OsString::from("x"))]
        .into_iter()
        .collect();

    assert_eq!(finish(Settings::new(&variables)).unread, ["RAIL_\u{fffd}"]);
}

#[test]
fn a_described_value_is_kept_as_given_and_a_warning_says_what_it_weakens() {
    let variables: Variables = [("A", "x")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    settings.describe("A", "a\u{1b}[31mb");
    settings.warn("B", true, "it weakens this");

    assert_eq!(
        finish(settings).entries,
        [
            entry("A", "a\u{1b}[31mb", Level::Info, Source::Env),
            entry("B", "true", Level::Warn("it weakens this"), Source::Default),
        ]
    );
}

#[test]
fn a_secret_is_described_only_as_set_or_unset() {
    let variables = Variables::default();
    let mut settings = Settings::new(&variables);

    settings.describe_secret("S", Some(&Secret::new("hunter2".to_owned())));
    settings.describe_secret("U", None);

    let values: Vec<_> = finish(settings)
        .entries
        .into_iter()
        .map(|entry| (entry.name, entry.value))
        .collect();
    assert_eq!(values, [("S", "set".to_owned()), ("U", "unset".to_owned())]);
}

#[test]
fn a_name_read_but_not_described_is_described_as_read() {
    let variables: Variables = [
        ("USED", "1"),
        ("UNUSED", " a\u{1b}[31mb\n"),
        ("TOKEN", "hunter2"),
        ("CHECKED", "yes"),
        ("BLANK", " "),
        ("OLD", ""),
        ("NEVER_READ", "x"),
    ]
    .into_iter()
    .collect();
    let mut settings = Settings::new(&variables);
    let _ = settings.get("USED").text();
    let _ = settings.get("UNUSED").text();
    let _ = settings.get("TOKEN").secret();
    settings.get("CHECKED").is_set();
    let _ = settings.get("BLANK").text();
    let _ = settings.get("UNSET").secret();
    settings.get("OLD").retired("nothing reads it");
    settings.describe("USED", "1 as used");

    assert_eq!(
        finish(settings).entries,
        [
            entry("USED", "1 as used", Level::Info, Source::Env),
            entry("UNUSED", "a\u{1b}[31mb", Level::Info, Source::Env),
            entry("TOKEN", "set", Level::Info, Source::Env),
            entry("CHECKED", "yes", Level::Info, Source::Env),
            entry("BLANK", "unset", Level::Info, Source::Default),
            entry("UNSET", "unset", Level::Info, Source::Default),
        ]
    );
}

#[test]
fn entries_follow_the_order_names_were_first_read() {
    let variables: Variables = [("A", "a"), ("B", "b")].into_iter().collect();
    let mut settings = Settings::new(&variables);
    let _ = settings.get("A").text();
    let _ = settings.get("B").text();
    settings.describe("B", "b1");
    settings.describe("C", "c");
    settings.describe("A", "a1");
    settings.describe("B", "b2");

    let entries: Vec<_> = finish(settings)
        .entries
        .into_iter()
        .map(|entry| (entry.name, entry.value))
        .collect();
    assert_eq!(
        entries,
        [
            ("A", "a1".to_owned()),
            ("B", "b1".to_owned()),
            ("B", "b2".to_owned()),
            ("C", "c".to_owned()),
        ]
    );
}

#[test]
fn a_refused_configuration_returns_only_its_refusals() {
    let variables: Variables = [("A", "x")].into_iter().collect();
    let mut settings = Settings::new(&variables);
    settings.describe("B", "b");
    let read = settings.get("A").boolean();

    assert_eq!(
        settings.finish("RAIL_", read.ok()),
        Err(ConfigErrors(vec![
            "A must be true or false, got: x".to_owned()
        ]))
    );
}
