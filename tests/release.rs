//! The version of the crate and the change log must agree. `scripts/release.sh` writes both.

#[test]
fn the_newest_version_in_the_change_log_is_the_version_of_the_crate() {
    let changelog = include_str!("../CHANGELOG.md");
    let newest = changelog
        .lines()
        .find_map(|line| line.strip_prefix("## "))
        .and_then(|heading| heading.split(' ').next());
    assert_eq!(newest, Some(env!("CARGO_PKG_VERSION")));
}
