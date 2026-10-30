use super::*;

#[test]
fn an_unwritable_report_is_found_before_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pyproject.toml");
    fs::write(&file, "").unwrap();
    assert!(writable(&file.join("out.xml")).is_err());
    assert!(writable(dir.path()).is_err());
    let nested = dir.path().join("reports/ci/out.xml");
    writable(&nested).unwrap();
    assert!(nested.is_file());
    // An existing report isn't truncated until it is rewritten.
    fs::write(&nested, "old").unwrap();
    writable(&nested).unwrap();
    assert_eq!(fs::read_to_string(&nested).unwrap(), "old");
}
