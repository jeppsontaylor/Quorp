# TUI Phase 1 dependency inventory (started March 29, 2026)

Phase 1 objective: classify unresolved workspace dependencies into `tui-critical`, `backend-shared`, or `legacy-gui` before adding any new local crates.

## Classification rules

- **tui-critical**: required directly by TUI runtime or ratatui pane behavior.
- **backend-shared**: required by backend services used by both TUI and GUI.
- **legacy-gui**: required only by GPUI/GUI surfaces and should be deferred from TUI unblock work.
- **unknown**: not yet validated from the quorp TUI compile path; requires dependency-edge proof before action.

## Initial inventory from `./script/stage-a-audit`

| Crate | Initial class | Rationale | Next action |
|---|---|---|---|
| assistant_slash_command | unknown | Assistant-related UI crate; TUI usage not yet proven. | Inspect quorp dependency edges and feature usage before creating local crate. |
| assistant_text_thread | unknown | Assistant thread UI naming; uncertain TUI dependency path. | Inspect dependency edges. |
| breadcrumbs | legacy-gui | Typical GUI navigation element. | Defer from TUI unblock. |
| collab_ui | unknown | Named UI crate but currently first hard blocker for `quorp`. | Prove whether TUI path requires it, then gate or implement minimal real crate. |
| command_palette | legacy-gui | Palette surface is generally GUI-first. | Defer/gate from TUI path. |
| component_preview | legacy-gui | Preview surface is GUI-facing. | Defer/gate from TUI path. |
| csv_preview | legacy-gui | Preview panel naming indicates GUI surface. | Defer/gate from TUI path. |
| dap_adapters | backend-shared | Debug adapter integrations can be backend shared. | Validate if quorp TUI links debugger services. |
| debugger_tools | backend-shared | Tooling may be shared service layer. | Validate dependency edge and keep only if needed. |
| debugger_ui | legacy-gui | Explicit UI crate. | Defer/gate from TUI path. |
| diagnostics | backend-shared | Diagnostics may be consumed by TUI and GUI. | Validate compile edge from TUI path. |
| editor | backend-shared | Core editor model may back preview logic. | Validate and keep if required for preview/open buffer behavior. |
| encoding_selector | legacy-gui | Selector UI naming. | Defer/gate from TUI path. |
| extensions_ui | legacy-gui | Explicit UI crate. | Defer/gate from TUI path. |
| feedback | legacy-gui | Product feedback UI flow. | Defer/gate from TUI path. |
| file_finder | unknown | Could map to command/search support used by TUI. | Validate quorp TUI edge before action. |
| go_to_line | unknown | Could be editor command shared by TUI. | Validate dependency edge before action. |
| image_viewer | legacy-gui | Viewer surface is GUI-first. | Defer/gate from TUI path. |
| inspector_ui | legacy-gui | Explicit UI crate. | Defer/gate from TUI path. |
| journal | unknown | Feature scope unclear from name. | Validate before action. |
| keymap_editor | legacy-gui | Editor UI surface. | Defer/gate from TUI path. |
| language_selector | legacy-gui | Selector UI surface. | Defer/gate from TUI path. |
| line_ending_selector | legacy-gui | Selector UI surface. | Defer/gate from TUI path. |
| markdown_preview | legacy-gui | Preview UI surface. | Defer/gate from TUI path. |
| notifications | backend-shared | Event channel may be shared; UI rendering should be optional. | Validate edge and keep core-only if required. |
| onboarding | legacy-gui | Onboarding UI flow. | Defer/gate from TUI path. |
| open_path_prompt | legacy-gui | Prompt UI flow. | Defer/gate from TUI path. |
| outline | unknown | Could be backend index + UI mixed. | Validate and split if needed. |
| outline_panel | legacy-gui | Explicit panel UI. | Defer/gate from TUI path. |
| picker | unknown | Could be generic utility or UI. | Validate before action. |
| platform_title_bar | legacy-gui | Explicit GUI title bar. | Defer/gate from TUI path. |
| project_panel | legacy-gui | Panel UI naming. | Defer/gate from TUI path. |
| recent_projects | unknown | Could be startup flow used by both modes. | Validate actual TUI dependency. |
| repl | unknown | Could be backend or UI depending on integration. | Validate before action. |
| search | backend-shared | Search engine is likely backend shared. | Validate and retain only backend dependency. |
| settings_profile_selector | legacy-gui | Selector UI surface. | Defer/gate from TUI path. |
| settings_ui | legacy-gui | Explicit UI crate. | Defer/gate from TUI path. |
| sidebar | legacy-gui | GUI container. | Defer/gate from TUI path. |
| snippets_ui | legacy-gui | Explicit UI crate. | Defer/gate from TUI path. |
| svg_preview | legacy-gui | Preview UI surface. | Defer/gate from TUI path. |
| tab_switcher | legacy-gui | GUI switching control. | Defer/gate from TUI path. |
| tasks_ui | legacy-gui | Explicit UI crate. | Defer/gate from TUI path. |
| terminal_view | legacy-gui | GPUI terminal view distinct from TUI terminal pane. | Defer/gate from TUI path. |
| theme_selector | legacy-gui | Selector UI surface. | Defer/gate from TUI path. |
| title_bar | legacy-gui | Explicit GUI title bar. | Defer/gate from TUI path. |
| tree-sitter-diff | backend-shared | Parser binding likely backend shared. | Validate if currently required for TUI compile path. |
| ui | legacy-gui | Generic GUI UI crate naming. | Defer/gate from TUI path. |
| vim | unknown | Could be backend editing mode used by TUI. | Validate edge from TUI code path. |
| vim_mode_setting | legacy-gui | Settings UI flavor by naming. | Defer/gate from TUI path. |
| which_key | unknown | Could be interaction helper used by TUI. | Validate before action. |
| workspace | backend-shared | Core workspace domain likely shared across modes. | Validate and keep backend-shared path only. |

## Phase 1 execution sequence

1. For each `unknown` and `backend-shared` entry, prove direct dependency edges from `quorp` TUI path.
2. Immediately gate or remove `legacy-gui` dependencies from the TUI-targeted compile graph.
3. Re-run `./script/stage-a-next-blocker` after each dependency-surface reduction.
4. Only add or resurrect crates when dependency-edge proof shows they are required for TUI-critical or backend-shared behavior.
5. Run `./script/tui-verify` after each batch to track progress toward Stage 2/3.
