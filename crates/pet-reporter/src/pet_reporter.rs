pub mod collect {
    use parking_lot::Mutex;
    use pet_core::python_environment::PythonEnvironment;

    #[derive(Default)]
    pub struct Reporter {
        pub environments: Mutex<Vec<PythonEnvironment>>,
    }

    pub fn create_reporter() -> Reporter {
        Reporter::default()
    }
}
