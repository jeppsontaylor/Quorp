use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Configuration {
    pub workspace_directories: Option<Vec<PathBuf>>,
}

pub mod os_environment {
    use std::path::PathBuf;

    pub trait Environment {
        fn get_user_home(&self) -> Option<PathBuf>;
        fn get_root(&self) -> Option<PathBuf>;
        fn get_env_var(&self, key: String) -> Option<String>;
        fn get_know_global_search_locations(&self) -> Vec<PathBuf>;
    }

    #[derive(Clone, Debug, Default)]
    pub struct EnvironmentApi;

    impl EnvironmentApi {
        pub fn new() -> Self {
            Self
        }
    }

    impl Environment for EnvironmentApi {
        fn get_user_home(&self) -> Option<PathBuf> {
            std::env::var_os("HOME").map(PathBuf::from)
        }

        fn get_root(&self) -> Option<PathBuf> {
            None
        }

        fn get_env_var(&self, key: String) -> Option<String> {
            std::env::var(key).ok()
        }

        fn get_know_global_search_locations(&self) -> Vec<PathBuf> {
            Vec::new()
        }
    }
}

pub mod python_environment {
    use super::*;

    #[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
    pub enum PythonEnvironmentKind {
        Conda,
        Poetry,
        Venv,
        Other,
    }

    #[derive(Clone, Debug, Serialize, Deserialize, Default)]
    pub struct PythonEnvironmentManager {
        pub executable: PathBuf,
    }

    #[derive(Clone, Debug, Serialize, Deserialize, Default)]
    pub struct PythonEnvironment {
        pub name: Option<String>,
        pub project: Option<PathBuf>,
        pub kind: Option<PythonEnvironmentKind>,
        pub manager: Option<PythonEnvironmentManager>,
        pub executable: PathBuf,
        pub prefix: Option<PathBuf>,
    }
}
