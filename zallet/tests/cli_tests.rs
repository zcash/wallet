#[test]
fn cli_tests() {
    let tests = trycmd::TestCases::new();

    tests.case("tests/cmd/*.toml");
}
