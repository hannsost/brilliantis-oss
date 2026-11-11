use brilliantis::{profile::Profile, Result};

/// Composition entry point. Internal endpoint and credential bindings are configured separately.
pub fn profile() -> Result<Profile> {
    Profile::load(
        include_str!("../profile.json"),
        include_str!("../demo.json"),
    )
}
