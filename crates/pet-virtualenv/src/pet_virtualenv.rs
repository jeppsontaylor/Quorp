use std::path::Path;

pub fn is_virtualenv_dir(path: &Path) -> bool {
    path.join("pyvenv.cfg").exists()
}
