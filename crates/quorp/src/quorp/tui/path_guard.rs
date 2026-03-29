//! Canonical path checks so tree selection and preview reads stay inside the project root.

use std::path::Path;

pub fn path_within_project(candidate: &Path, project_root: &Path) -> bool {
    match (candidate.canonicalize(), project_root.canonicalize()) {
        (Ok(p), Ok(r)) => p.strip_prefix(&r).is_ok(),
        _ => false,
    }
}
