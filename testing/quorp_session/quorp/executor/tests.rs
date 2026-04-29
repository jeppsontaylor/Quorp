use super::*;
use std::sync::Mutex;

static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn parse_provider_supports_nvidia_aliases() {
    assert_eq!(
        parse_provider("nvidia"),
        Some(InteractiveProviderKind::Nvidia)
    );
    assert_eq!(parse_provider("nim"), Some(InteractiveProviderKind::Nvidia));
    assert_eq!(
        parse_provider("nvidia-nim"),
        Some(InteractiveProviderKind::Nvidia)
    );
}

#[test]
fn interactive_provider_accepts_nvidia_env() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("QUORP_PROVIDER", "nvidia");
    }
    assert_eq!(
        interactive_provider_from_env(),
        InteractiveProviderKind::Nvidia
    );
    unsafe {
        std::env::remove_var("QUORP_PROVIDER");
    }
}
