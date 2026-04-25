use std::path::Path;

pub fn path_within_project(candidate: &Path, project_root: &Path) -> bool {
    match (candidate.canonicalize(), project_root.canonicalize()) {
        (Ok(path), Ok(root)) => path.strip_prefix(&root).is_ok(),
        _ => false,
    }
}
