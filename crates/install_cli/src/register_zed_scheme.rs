use client::QUORP_URL_SCHEME;
use gpui::{AsyncApp, actions};

actions!(
    cli,
    [
        /// Registers the quorp:// URL scheme handler.
        RegisterQuorpScheme
    ]
);

pub async fn register_quorp_scheme(cx: &AsyncApp) -> anyhow::Result<()> {
    cx.update(|cx| cx.register_url_scheme(QUORP_URL_SCHEME)).await
}
