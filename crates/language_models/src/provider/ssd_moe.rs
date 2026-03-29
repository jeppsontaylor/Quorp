#![allow(unused)]
use anyhow::Result;
use futures::{FutureExt, StreamExt, future::BoxFuture};
use gpui::{AnyView, App, AsyncApp, Context, Entity, SharedString, Task, Window, Render};
use http_client::HttpClient;
use language_model::{
    IconOrSvg, LanguageModel, 
    LanguageModelCompletionEvent, LanguageModelId, LanguageModelName, LanguageModelProvider,
    LanguageModelProviderId, LanguageModelProviderName, LanguageModelProviderState,
    LanguageModelRequest, LanguageModelToolChoice, LanguageModelToolSchemaFormat, RateLimiter,
};
use open_ai::{
    ResponseStreamEvent,
    responses::{Request as ResponseRequest, StreamEvent as ResponsesStreamEvent, stream_response},
    stream_completion,
};
use settings::Settings;
use std::sync::{Arc, OnceLock, RwLock};
use ui::prelude::*;

use crate::provider::open_ai::{
    OpenAiEventMapper, into_open_ai, collect_tiktoken_messages,
};
use crate::provider::ssd_moe_server::{SsdMoeServer, ServerStatus};
use crate::AllLanguageModelSettings;

static SSD_MOE_SHARED_SERVER: OnceLock<Arc<RwLock<SsdMoeServer>>> = OnceLock::new();

/// Process-local singleton used by the SSD-MOE provider and the TUI so one `infer` process is shared.
pub fn ssd_moe_shared_server() -> Option<Arc<RwLock<SsdMoeServer>>> {
    SSD_MOE_SHARED_SERVER.get().cloned()
}

#[derive(Clone, Debug, PartialEq)]
pub struct SsdMoeSettings {
    pub server_path: Option<String>,
    pub port: u16,
    pub model_dir: String,
    pub k_experts: u32,
}

pub struct SsdMoeLanguageModelProvider {
    id: LanguageModelProviderId,
    name: LanguageModelProviderName,
    http_client: Arc<dyn HttpClient>,
    state: Entity<State>,
}

pub struct State {
    server: Arc<RwLock<SsdMoeServer>>,
    api_url: String,
    model_name: String,
}

impl State {
    fn is_authenticated(&self) -> bool {
        self.server.read().unwrap().status() == ServerStatus::Ready
    }

    fn authenticate(
        &mut self,
        _http_client: Arc<dyn HttpClient>,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<(), language_model::AuthenticateError>> {
        let mut server = self.server.write().unwrap();
        let settings = AllLanguageModelSettings::get_global(cx).ssd_moe.clone();

        server.start(
            settings.server_path.clone(),
            settings.port,
            &settings.model_dir,
            settings.k_experts,
            None,
        );
        
        // Return a task that waits for the server to be Ready or Failed
        let server_ref = self.server.clone();
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let status = server_ref.read().unwrap().status();
                match status {
                    ServerStatus::Ready => return Ok(()),
                    ServerStatus::Failed(e) => return Err(language_model::AuthenticateError::Other(anyhow::anyhow!(e))),
                    _ => {}
                }
            }
        })
    }
}

impl SsdMoeLanguageModelProvider {
    pub fn new(http_client: Arc<dyn HttpClient>, cx: &mut App) -> Self {
        let settings = AllLanguageModelSettings::get_global(cx).ssd_moe.clone();
        let port = settings.port;
        let server = SSD_MOE_SHARED_SERVER
            .get_or_init(|| Arc::new(RwLock::new(SsdMoeServer::new(port))))
            .clone();
        
        let state = cx.new(|_| State {
            server,
            api_url: format!("http://127.0.0.1:{}/v1", port),
            model_name: "qwen3.5-35b-a3b".to_string(),
        });

        Self {
            id: LanguageModelProviderId::from("ssd_moe".to_string()),
            name: LanguageModelProviderName::from("SSD-MOE".to_string()),
            http_client,
            state,
        }
    }

    fn create_language_model(&self) -> Arc<dyn LanguageModel> {
        Arc::new(SsdMoeLanguageModel {
            id: LanguageModelId::from("qwen3.5-35b-a3b".to_string()),
            provider_id: self.id.clone(),
            provider_name: self.name.clone(),
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }
}

impl LanguageModelProviderState for SsdMoeLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for SsdMoeLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelProviderName {
        self.name.clone()
    }

    fn icon(&self) -> IconOrSvg {
        IconOrSvg::Icon(IconName::Sparkle)
    }

    fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model())
    }

    fn default_fast_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        None
    }

    fn provided_models(&self, _cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        vec![self.create_language_model()]
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<anyhow::Result<(), language_model::AuthenticateError>> {
        let http_client = self.http_client.clone();
        self.state.update(cx, |state, cx| state.authenticate(http_client, cx))
    }

    fn configuration_view(
        &self,
        _target_agent: language_model::ConfigurationViewTargetAgent,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|cx| ConfigurationView::new(self.state.clone(), self.http_client.clone(), window, cx))
            .into()
    }

    fn reset_credentials(&self, _cx: &mut App) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
}

pub struct SsdMoeLanguageModel {
    id: LanguageModelId,
    provider_id: LanguageModelProviderId,
    provider_name: LanguageModelProviderName,
    state: Entity<State>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl SsdMoeLanguageModel {
    fn stream_completion(
        &self,
        request: open_ai::Request,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        anyhow::Result<
            futures::stream::BoxStream<'static, anyhow::Result<ResponseStreamEvent>>,
            language_model::LanguageModelCompletionError,
        >,
    > {
        let http_client = self.http_client.clone();
        let api_url = self.state.read_with(cx, |state, _| state.api_url.clone());
        let provider = self.provider_name.clone();

        let future = self.request_limiter.stream(async move {
            let request = stream_completion(
                http_client.as_ref(),
                provider.0.as_str(),
                &api_url,
                "local", // No API key required for local
                request,
            );
            Ok(request.await?)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }

    fn stream_response(
        &self,
        request: ResponseRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, anyhow::Result<futures::stream::BoxStream<'static, anyhow::Result<ResponsesStreamEvent>>>>
    {
        let http_client = self.http_client.clone();
        let api_url = self.state.read_with(cx, |state, _| state.api_url.clone());
        let provider = self.provider_name.clone();

        let future = self.request_limiter.stream(async move {
            let request = stream_response(
                http_client.as_ref(),
                provider.0.as_str(),
                &api_url,
                "local",
                request,
            );
            Ok(request.await?)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }
}

impl LanguageModel for SsdMoeLanguageModel {
    fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName(SharedString::from("Flash-MOE Qwen 35B"))
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        self.provider_id.clone()
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        self.provider_name.clone()
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn tool_input_format(&self) -> LanguageModelToolSchemaFormat {
        LanguageModelToolSchemaFormat::JsonSchemaSubset
    }

    fn supports_images(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    fn supports_tool_choice(&self, _choice: LanguageModelToolChoice) -> bool {
        false
    }

    fn supports_streaming_tools(&self) -> bool {
        false
    }

    fn supports_split_token_display(&self) -> bool {
        true
    }

    fn telemetry_id(&self) -> String {
        "ssd_moe/qwen3.5-35b-a3b".to_string()
    }

    fn max_token_count(&self) -> u64 {
        32768
    }

    fn max_output_tokens(&self) -> Option<u64> {
        Some(8192)
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        cx: &App,
    ) -> BoxFuture<'static, anyhow::Result<u64>> {
        cx.background_spawn(async move {
            let messages = collect_tiktoken_messages(request);
            tiktoken_rs::num_tokens_from_messages("gpt-4", &messages).map(|t| t as u64)
        })
        .boxed()
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        anyhow::Result<
            futures::stream::BoxStream<
                'static,
                anyhow::Result<LanguageModelCompletionEvent, language_model::LanguageModelCompletionError>,
            >,
            language_model::LanguageModelCompletionError,
        >,
    > {
        let request = into_open_ai(
            request,
            &self.id.0,
            false,
            false,
            self.max_output_tokens(),
            None,
        );
        let completions = self.stream_completion(request, cx);
        async move {
            let mapper = OpenAiEventMapper::new();
            Ok(mapper.map_stream(completions.await?).boxed())
        }
        .boxed()
    }
}

struct ConfigurationView {
    state: Entity<State>,
    http_client: Arc<dyn HttpClient>,
    _refresh_task: Task<()>,
}

impl ConfigurationView {
    fn new(state: Entity<State>, http_client: Arc<dyn HttpClient>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Start the server if it's currently stopped
        let is_stopped = state.read(cx).server.read().unwrap().status() == ServerStatus::Stopped;
        if is_stopped {
            state.update(cx, |s, cx| {
                let _ = s.authenticate(http_client.clone(), cx);
            });
        }

        // Poll logs and status to re-render
        let state_ref = state.clone();
        let refresh_task = cx.spawn_in(window, async move |_, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let result = state_ref.update(cx, |_, cx| {
                    cx.notify();
                    Ok::<(), anyhow::Error>(())
                });
                if result.is_err() {
                    break;
                }
            }
        });

        Self { state, http_client, _refresh_task: refresh_task }
    }

    fn restart_server(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.server.write().unwrap().stop();
            let _ = s.authenticate(self.http_client.clone(), cx);
        });
    }

    fn open_log_file(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let path = self.state.read(cx).server.read().unwrap().log_file_path().to_path_buf();
        let formatted = format!("file://{}", path.display());
        cx.open_url(&formatted);
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let server_ref = state.server.read().unwrap();
        let status = server_ref.status();
        let logs = server_ref.get_logs();

        let status_label = match status {
            ServerStatus::Stopped => "Stopped",
            ServerStatus::Starting => "Starting... (Loading Weights)",
            ServerStatus::Ready => "Ready",
            ServerStatus::Failed(_) => "Failed",
        };

        let status_color = match status {
            ServerStatus::Stopped => Color::Muted,
            ServerStatus::Starting => Color::Warning,
            ServerStatus::Ready => Color::Success,
            ServerStatus::Failed(_) => Color::Error,
        };

        let log_content = if logs.is_empty() {
            "No logs yet...".to_string()
        } else {
            logs.join("\n")
        };

        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        h_flex().gap_2()
                            .child(Icon::new(IconName::Sparkle).color(status_color))
                            .child(Label::new(format!("Server Status: {}", status_label)).color(status_color))
                    )
                    .child(
                        h_flex().gap_2()
                            .child(
                                Button::new("open-logs", "Open Log File")
                                    .on_click(cx.listener(|this, _, window, cx| this.open_log_file(window, cx)))
                            )
                            .child(
                                Button::new("restart-server", "Restart Server")
                                    .on_click(cx.listener(|this, _, window, cx| this.restart_server(window, cx)))
                            )
                    )
            )
            .child(
                div()
                    .w_full()
                    .h_64()
                    .bg(cx.theme().colors().editor_background)
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .rounded_md()
                    .p_2()
                    .overflow_hidden()
                    .child(
                        Label::new(log_content)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                    )
            )
            .when_some(
                if let ServerStatus::Failed(msg) = status { Some(msg) } else { None },
                |el, msg| {
                    el.child(
                        div()
                            .mt_2()
                            .p_2()
                            .bg(cx.theme().status().error_background)
                            .rounded_md()
                            .child(Label::new(format!("Error: {}", msg)).color(Color::Error))
                    )
                }
            )
            .into_any()
    }
}
