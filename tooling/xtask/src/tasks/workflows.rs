use anyhow::{Context, Result};
use clap::Parser;
use gh_workflow::Workflow;
use std::fs;
use std::path::{Path, PathBuf};

use crate::tasks::workflow_checks::{self};

mod after_release;
mod autofix_pr;
mod bump_patch_version;
mod cherry_pick;
mod compare_perf;
mod danger;
mod deploy_collab;
mod extension_auto_bump;
mod extension_bump;
mod extension_tests;
mod extension_workflow_rollout;
mod extensions;
mod nix_build;
mod publish_extension_cli;
mod release_nightly;
mod run_bundling;

mod release;
mod run_agent_evals;
mod run_tests;
mod runners;
mod steps;
mod vars;

#[derive(Clone)]
pub(crate) struct GitSha(String);

impl AsRef<str> for GitSha {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "This runs only in a CLI environment"
)]
fn parse_ref(value: &str) -> Result<GitSha, String> {
    const GIT_SHA_LENGTH: usize = 40;
    (value.len() == GIT_SHA_LENGTH)
        .then_some(value)
        .ok_or_else(|| {
            format!(
                "Git SHA has wrong length! \
                Only SHAs with a full length of {GIT_SHA_LENGTH} are supported, found {len} characters.",
                len = value.len()
            )
        })
        .and_then(|value| {
            let mut tmp = [0; 4];
            value
                .chars()
                .all(|char| u16::from_str_radix(char.encode_utf8(&mut tmp), 16).is_ok()).then_some(value)
                .ok_or_else(|| "Not a valid Git SHA".to_owned())
        })
        .and_then(|sha| {
           std::process::Command::new("git")
               .args([
                   "rev-parse",
                   "--quiet",
                   "--verify",
                   &format!("{sha}^{{commit}}")
               ])
               .output()
               .map_err(|_| "Failed to spawn Git command to verify SHA".to_owned())
               .and_then(|output|
                   output
                       .status.success()
                       .then_some(sha)
                       .ok_or_else(|| format!("SHA {sha} is not a valid Git SHA within this repository!")))
        }).map(|sha| GitSha(sha.to_owned()))
}

#[derive(Parser)]
pub(crate) struct GenerateWorkflowArgs {
    #[arg(value_parser = parse_ref)]
    /// The Git SHA to use when invoking this
    pub(crate) sha: Option<GitSha>,
}

enum WorkflowSource {
    Contextless(fn() -> Workflow),
    WithContext(fn(&GenerateWorkflowArgs) -> Workflow),
}

struct WorkflowFile {
    source: WorkflowSource,
    r#type: WorkflowType,
}

impl WorkflowFile {
    fn quorp(f: fn() -> Workflow) -> WorkflowFile {
        WorkflowFile {
            source: WorkflowSource::Contextless(f),
            r#type: WorkflowType::Quorp,
        }
    }

    fn extension(f: fn(&GenerateWorkflowArgs) -> Workflow) -> WorkflowFile {
        WorkflowFile {
            source: WorkflowSource::WithContext(f),
            r#type: WorkflowType::ExtensionCi,
        }
    }

    fn extension_shared(f: fn(&GenerateWorkflowArgs) -> Workflow) -> WorkflowFile {
        WorkflowFile {
            source: WorkflowSource::WithContext(f),
            r#type: WorkflowType::ExtensionsShared,
        }
    }

    fn generate_file(&self, workflow_args: &GenerateWorkflowArgs) -> Result<()> {
        let workflow = match &self.source {
            WorkflowSource::Contextless(f) => f(),
            WorkflowSource::WithContext(f) => f(workflow_args),
        };
        let workflow_folder = self.r#type.folder_path();

        fs::create_dir_all(&workflow_folder).with_context(|| {
            format!("Failed to create directory: {}", workflow_folder.display())
        })?;

        let workflow_name = workflow
            .name
            .as_ref()
            .expect("Workflow must have a name at this point");
        let filename = format!(
            "{}.yml",
            workflow_name.rsplit("::").next().unwrap_or(workflow_name)
        );

        let workflow_path = workflow_folder.join(filename);

        let content = workflow
            .to_string()
            .map_err(|e| anyhow::anyhow!("{:?}: {:?}", workflow_path, e))?;

        let disclaimer = self.r#type.disclaimer(workflow_name);

        let content = [disclaimer, content].join("\n");
        fs::write(&workflow_path, content).map_err(Into::into)
    }
}

#[derive(PartialEq, Eq, strum::EnumIter)]
pub enum WorkflowType {
    /// Workflows living in the Quorp repository
    Quorp,
    /// Workflows living in the `quorp-extensions/workflows` repository that are
    /// required workflows for PRs to the extension organization
    ExtensionCi,
    /// Workflows living in each of the extensions to perform checks and version
    /// bumps until a better, more centraliquorp system for that is in place.
    ExtensionsShared,
}

impl WorkflowType {
    fn disclaimer(&self, workflow_name: &str) -> String {
        format!(
            concat!(
                "# Generated from xtask::workflows::{}{}\n",
                "# Rebuild with `cargo xtask workflows`.",
            ),
            workflow_name,
            (*self != WorkflowType::Quorp)
                .then_some(" within the Quorp repository.")
                .unwrap_or_default(),
        )
    }

    pub fn folder_path(&self) -> PathBuf {
        match self {
            WorkflowType::Quorp => PathBuf::from(".github/workflows"),
            WorkflowType::ExtensionCi => PathBuf::from("extensions/workflows"),
            WorkflowType::ExtensionsShared => PathBuf::from("extensions/workflows/shared"),
        }
    }
}

pub fn run_workflows(args: GenerateWorkflowArgs) -> Result<()> {
    if !Path::new("crates/quorp/").is_dir() {
        anyhow::bail!("xtask workflows must be ran from the project root");
    }

    let workflows = [
        WorkflowFile::quorp(after_release::after_release),
        WorkflowFile::quorp(autofix_pr::autofix_pr),
        WorkflowFile::quorp(bump_patch_version::bump_patch_version),
        WorkflowFile::quorp(cherry_pick::cherry_pick),
        WorkflowFile::quorp(compare_perf::compare_perf),
        WorkflowFile::quorp(danger::danger),
        WorkflowFile::quorp(deploy_collab::deploy_collab),
        WorkflowFile::quorp(extension_bump::extension_bump),
        WorkflowFile::quorp(extension_auto_bump::extension_auto_bump),
        WorkflowFile::quorp(extension_tests::extension_tests),
        WorkflowFile::quorp(extension_workflow_rollout::extension_workflow_rollout),
        WorkflowFile::quorp(publish_extension_cli::publish_extension_cli),
        WorkflowFile::quorp(release::release),
        WorkflowFile::quorp(release_nightly::release_nightly),
        WorkflowFile::quorp(run_agent_evals::run_agent_evals),
        WorkflowFile::quorp(run_agent_evals::run_cron_unit_evals),
        WorkflowFile::quorp(run_agent_evals::run_unit_evals),
        WorkflowFile::quorp(run_bundling::run_bundling),
        WorkflowFile::quorp(run_tests::run_tests),
        /* workflows used for CI/CD in extension repositories */
        WorkflowFile::extension(extensions::run_tests::run_tests),
        WorkflowFile::extension_shared(extensions::bump_version::bump_version),
    ];

    for workflow_file in workflows {
        workflow_file.generate_file(&args)?;
    }

    workflow_checks::validate(Default::default())
}
