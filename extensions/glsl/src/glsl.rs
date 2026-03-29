use std::fs;
use quorp::settings::LspSettings;
use quorp_extension_api::{self as quorp, LanguageServerId, Result, serde_json};

struct GlslExtension {
    cached_binary_path: Option<String>,
}

impl GlslExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &quorp::Worktree,
    ) -> Result<String> {
        if let Some(path) = worktree.which("glsl_analyzer") {
            return Ok(path);
        }

        if let Some(path) = &self.cached_binary_path
            && fs::metadata(path).is_ok_and(|stat| stat.is_file())
        {
            return Ok(path.clone());
        }

        quorp::set_language_server_installation_status(
            language_server_id,
            &quorp::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = quorp::latest_github_release(
            "nolanderc/glsl_analyzer",
            quorp::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (platform, arch) = quorp::current_platform();
        let asset_name = format!(
            "{arch}-{os}.zip",
            arch = match arch {
                quorp::Architecture::Aarch64 => "aarch64",
                quorp::Architecture::X86 => "x86",
                quorp::Architecture::X8664 => "x86_64",
            },
            os = match platform {
                quorp::Os::Mac => "macos",
                quorp::Os::Linux => "linux-musl",
                quorp::Os::Windows => "windows",
            }
        );

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no asset found matching {:?}", asset_name))?;

        let version_dir = format!("glsl_analyzer-{}", release.version);
        fs::create_dir_all(&version_dir)
            .map_err(|err| format!("failed to create directory '{version_dir}': {err}"))?;
        let binary_path = format!("{version_dir}/bin/glsl_analyzer");

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            quorp::set_language_server_installation_status(
                language_server_id,
                &quorp::LanguageServerInstallationStatus::Downloading,
            );

            quorp::download_file(
                &asset.download_url,
                &version_dir,
                match platform {
                    quorp::Os::Mac | quorp::Os::Linux => quorp::DownloadedFileType::Zip,
                    quorp::Os::Windows => quorp::DownloadedFileType::Zip,
                },
            )
            .map_err(|e| format!("failed to download file: {e}"))?;

            quorp::make_file_executable(&binary_path)?;

            let entries =
                fs::read_dir(".").map_err(|e| format!("failed to list working directory {e}"))?;
            for entry in entries {
                let entry = entry.map_err(|e| format!("failed to load directory entry {e}"))?;
                if entry.file_name().to_str() != Some(&version_dir) {
                    fs::remove_dir_all(entry.path()).ok();
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl quorp::Extension for GlslExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &quorp::LanguageServerId,
        worktree: &quorp::Worktree,
    ) -> Result<quorp::Command> {
        Ok(quorp::Command {
            command: self.language_server_binary_path(language_server_id, worktree)?,
            args: vec![],
            env: Default::default(),
        })
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &quorp::LanguageServerId,
        worktree: &quorp::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let settings = LspSettings::for_worktree("glsl_analyzer", worktree)
            .ok()
            .and_then(|lsp_settings| lsp_settings.settings)
            .unwrap_or_default();

        Ok(Some(serde_json::json!({
            "glsl_analyzer": settings
        })))
    }
}

quorp::register_extension!(GlslExtension);
