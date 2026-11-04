use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use rail_runtime::{ConfigErrors, EXIT_CODE, Settings, Variables};

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
fn a_value_past_its_length_is_refused_with_the_reason_for_the_bound() {
    let variables: Variables = [("H", "abcdé"), ("S", "abcd")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    assert_eq!(
        settings.get("S").max_chars(4, "a host reads").text(),
        Some("abcd")
    );
    assert_eq!(settings.get("H").max_chars(4, "a host reads").text(), None);
    assert_eq!(
        settings.finish().unwrap_err().0,
        ["H has 5 characters, past the 4 a host reads"]
    );
}

#[test]
fn a_refusal_and_a_retired_name_follow_the_name() {
    let variables: Variables = [("OLD", "1")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    settings.get("OLD").retired("NEW replaces it");
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
