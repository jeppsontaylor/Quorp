use super::*;

#[test]
fn default_decay_per_tier() {
    assert_eq!(Tier::Working.default_decay(), DecayPolicy::Fast);
    assert_eq!(Tier::Procedural.default_decay(), DecayPolicy::Never);
}
