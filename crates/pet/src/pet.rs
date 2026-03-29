use anyhow::Result;
use pet_core::os_environment::Environment;
use pet_core::python_environment::PythonEnvironment;
use std::path::Path;
use std::sync::Arc;

pub mod locators {
    use super::*;
    use pet_core::Configuration;

    #[derive(Clone, Debug, Default)]
    pub struct LocatorHandle;

    impl LocatorHandle {
        pub fn configure(&self, _configuration: &Configuration) {}
    }

    pub fn create_locators<C, P, E>(
        _conda: Arc<C>,
        _poetry: Arc<P>,
        _environment: &E,
    ) -> Vec<LocatorHandle>
    where
        C: Send + Sync + 'static,
        P: Send + Sync + 'static,
    {
        vec![LocatorHandle, LocatorHandle]
    }
}

pub mod find {
    use super::*;

    pub fn find_and_report_envs<E>(
        _reporter: &pet_reporter::collect::Reporter,
        _configuration: pet_core::Configuration,
        _locators: &[locators::LocatorHandle],
        _environment: &E,
        _workspace: Option<&Path>,
    ) {
    }
}

pub mod resolve {
    use super::*;

    #[derive(Clone, Debug)]
    pub struct ResolvedEnvironment {
        pub discovered: PythonEnvironment,
        pub resolved: Option<PythonEnvironment>,
    }

    pub fn resolve_environment<E>(
        path: &Path,
        _locators: &[locators::LocatorHandle],
        _environment: &E,
    ) -> Result<ResolvedEnvironment>
    where
        E: Environment,
    {
        let executable = if path.is_dir() {
            path.join("bin/python")
        } else {
            path.to_path_buf()
        };
        Ok(ResolvedEnvironment {
            discovered: PythonEnvironment {
                executable,
                prefix: path.parent().map(|parent| parent.to_path_buf()),
                ..Default::default()
            },
            resolved: None,
        })
    }
}
