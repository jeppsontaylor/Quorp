use super::*;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn memory_log_path_uses_logs_directory() {
    assert_eq!(memory_log_file(), &logs_dir().join("QuorpMemory.log"));
}

#[test]
fn user_models_dir_prefers_gary_models_dir() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::set_var(GARY_MODELS_DIR_VAR, "/tmp/gary-models");
    }

    assert_eq!(user_models_dir(), PathBuf::from("/tmp/gary-models"));

    unsafe {
        std::env::remove_var(GARY_MODELS_DIR_VAR);
    }
}

#[test]
fn user_models_dir_defaults_to_moe_root() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::remove_var(GARY_MODELS_DIR_VAR);
        std::env::remove_var(SSD_MOE_MODELS_DIR_VAR);
    }

    assert_eq!(user_models_dir(), PathBuf::from("/Volumes/MOE/models"));
}

#[test]
fn ssd_moe_state_dir_defaults_to_shared_local_state() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::remove_var(QUORP_SSD_MOE_STATE_DIR_VAR);
        std::env::remove_var(SSD_MOE_STATE_DIR_VAR);
    }

    assert_eq!(
        ssd_moe_state_dir(),
        home_dir().join(".local").join("state").join("ssd-moe")
    );
}
