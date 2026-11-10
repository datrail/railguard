use rail_test_support::wait_until;

// A test ending on `wait_until` checks nothing if it returns before the condition holds.
#[test]
fn wait_until_polls_until_the_condition_holds() {
    let mut polls = 0;

    wait_until(|| {
        polls += 1;
        polls == 3
    });

    assert_eq!(polls, 3);
}
