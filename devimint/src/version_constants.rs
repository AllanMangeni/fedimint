use std::sync::LazyLock;

use semver::Version;

pub static VERSION_0_8_0_ALPHA: LazyLock<Version> =
    LazyLock::new(|| Version::parse("0.8.0-alpha").expect("version is parsable"));
pub static VERSION_0_9_0_ALPHA: LazyLock<Version> =
    LazyLock::new(|| Version::parse("0.9.0-alpha").expect("version is parsable"));
