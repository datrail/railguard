use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use rail_runtime::{ConfigErrors, EXIT_CODE, Settings, Variables};

#[test]
fn text_is_trimmed_and_blank_is_unset() {
    let variables: Variables = [("A", " a \n"), ("B", " \t"), ("C", "")]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert_eq!(settings.text("A"), Some("a"));
    assert_eq!(settings.text("B"), None);
    assert_eq!(settings.text("C"), None);
    assert_eq!(settings.text("D"), None);
    assert!(settings.is_set("A"));
    assert!(!settings.is_set("B"));
    assert_eq!(settings.finish(), Ok(()));
}

#[test]
fn a_value_that_is_not_utf8_is_refused_once() {
    let variables: Variables = [(OsString::from("A"), OsString::from_vec(vec![0xff]))]
        .into_iter()
        .collect();
    let mut settings = Settings::new(&variables);

    assert!(settings.is_set("A"));
    assert_eq!(settings.required("A"), None);
    assert_eq!(settings.secret("A"), None);

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
    assert_eq!(settings.parse("N", number), None);
    assert_eq!(settings.parse("M", number), None);
    assert_eq!(settings.parse("UNSET", number), None);
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

    assert!(settings.boolean("T", false));
    assert!(!settings.boolean("F", true));
    assert!(settings.boolean("UNSET", true));
    assert!(!settings.boolean("X", false));
    assert_eq!(
        settings.finish().unwrap_err().0,
        ["X must be true or false, got: 1"]
    );
}

#[test]
fn a_refused_value_is_quoted_safely() {
    let variables: Variables = [("X", "a\u{1b}[31mb")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    settings.boolean("X", false);

    assert_eq!(
        settings.finish().unwrap_err().0,
        ["X must be true or false, got: a\\u{1b}[31mb"]
    );
}

#[test]
fn a_secret_is_trimmed_and_never_debugs() {
    let variables: Variables = [("S", "hunter2\n")].into_iter().collect();
    let mut settings = Settings::new(&variables);

    let secret = settings.secret("S").unwrap();

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

    assert_eq!(settings.required_parse("P", port), Some(8080));
    assert_eq!(settings.required_parse("N", port), None);
    assert_eq!(settings.required_parse("UNSET", port), None);
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

    settings.text("A");
    let before = settings.refusals();
    settings.text("A");

    assert!(settings.refusals() > before);
    assert_eq!(settings.finish().unwrap_err().0, ["A is not valid UTF-8"]);
}
