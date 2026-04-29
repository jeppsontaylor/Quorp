use super::*;

#[test]
fn trusted_project_can_elevate_permissions() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");

    let user_path = temp_dir.path().join("user.json");
    let project_path = workspace.join(".quorp").join("settings.json");
    let legacy_path = workspace.join(".quorp").join("agent.toml");
    fs::create_dir_all(project_path.parent().expect("parent")).expect("config dir");

    let project_id = canonical_project_key(&workspace);
    fs::write(
        &user_path,
        format!(
            r#"{{
              "trust": {{
                "trusted_projects": ["{project_id}"]
              }},
              "permissions": {{
                "mode": "ask"
              }},
              "sandbox": {{
                "mode": "tmp-copy"
              }}
            }}"#
        ),
    )
    .expect("user");
    fs::write(
        &project_path,
        r#"{
          "permissions": {
            "mode": "full-permissions"
          },
          "sandbox": {
            "mode": "host"
          }
        }"#,
    )
    .expect("project");

    let loaded = load_settings_from_paths(&workspace, &user_path, &project_path, &legacy_path)
        .expect("settings");

    assert!(loaded.trust.trusted);
    assert_eq!(
        loaded.settings.permissions.mode,
        PermissionMode::FullPermissions
    );
    assert_eq!(loaded.settings.sandbox.mode, SandboxMode::Host);
}

#[test]
fn untrusted_project_cannot_elevate_permissions_or_network() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path().join("workspace");
    fs::create_dir_all(workspace.join(".quorp")).expect("workspace");

    let user_path = temp_dir.path().join("user.json");
    let project_path = workspace.join(".quorp").join("settings.json");
    let legacy_path = workspace.join(".quorp").join("agent.toml");
    fs::write(
        &user_path,
        r#"{
          "permissions": {
            "mode": "ask",
            "allow_network": false
          },
          "mcp": {
            "enabled": false
          },
          "tools": {
            "browser": false
          }
        }"#,
    )
    .expect("user");
    fs::write(
        &project_path,
        r#"{
          "permissions": {
            "mode": "full-permissions",
            "allow_network": true,
            "allow_mcp": true,
            "allow_browser": true,
            "allow_process_control": true
          },
          "mcp": {
            "enabled": true,
            "allowed_servers": ["docs"]
          },
          "tools": {
            "browser": true,
            "mcp": true,
            "process_control": true
          },
          "sandbox": {
            "mode": "host"
          }
        }"#,
    )
    .expect("project");

    let loaded = load_settings_from_paths(&workspace, &user_path, &project_path, &legacy_path)
        .expect("settings");

    assert!(!loaded.trust.trusted);
    assert_eq!(loaded.settings.permissions.mode, PermissionMode::Ask);
    assert!(!loaded.settings.permissions.allow_network);
    assert!(!loaded.settings.permissions.allow_mcp);
    assert!(!loaded.settings.permissions.allow_browser);
    assert!(!loaded.settings.permissions.allow_process_control);
    assert!(!loaded.settings.mcp.enabled);
    assert!(!loaded.settings.tools.browser);
    assert_eq!(loaded.settings.sandbox.mode, SandboxMode::TmpCopy);
}

#[test]
fn doctor_surface_warns_about_legacy_agent_toml() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let workspace = temp_dir.path().join("workspace");
    fs::create_dir_all(workspace.join(".quorp")).expect("workspace");

    let user_path = temp_dir.path().join("user.json");
    let project_path = workspace.join(".quorp").join("settings.json");
    let legacy_path = workspace.join(".quorp").join("agent.toml");
    fs::write(&legacy_path, "[defaults]\nmode = \"act\"\n").expect("legacy");

    let loaded = load_settings_from_paths(&workspace, &user_path, &project_path, &legacy_path)
        .expect("settings");
    assert!(loaded.sources.loaded_legacy_agent_toml);
    assert!(
        loaded
            .warnings
            .iter()
            .any(|warning| warning.contains("settings.json is canonical"))
    );
}

#[test]
fn sandbox_runtime_round_trips() {
    let settings = Settings {
        sandbox: SandboxSettings {
            mode: SandboxMode::Host,
            keep_last_sandbox: true,
            runtime: SandboxRuntimeSettings {
                profile: quorp_core::SandboxRuntimeProfile::Container,
                container: quorp_core::ContainerRuntimeSettings {
                    engine: quorp_core::ContainerEnginePreference::Docker,
                    image: "example/container:latest".to_string(),
                },
            },
        },
        ..Settings::default()
    };
    let json = serde_json::to_string(&settings).expect("serialize");
    let round_tripped: Settings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        round_tripped.sandbox.runtime.profile,
        quorp_core::SandboxRuntimeProfile::Container
    );
    assert_eq!(
        round_tripped.sandbox.runtime.container.image,
        "example/container:latest"
    );
}
