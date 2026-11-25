mod domain;
mod fingerprint;
mod host;
mod service;
mod state_codec;

use gateway_plugin_sdk::{
    CallBudgets, Stage,
    client::{PluginBuilder, PluginSession, SessionConfig, methods},
};
use std::sync::Arc;

const AUTHOR_MANIFEST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/plugin.json"));

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let version = serde_json::from_slice::<serde_json::Value>(AUTHOR_MANIFEST)?["version"]
        .as_str()
        .ok_or("missing version")?
        .to_owned();
    let directory = std::env::current_exe()?
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("binary is outside the package")?
        .join("data");
    // On a broken bank, keep the settings/history page available but never send a probe.
    let service = Arc::new(service::Service::new(
        version,
        fingerprint::Fingerprint::load(&directory).ok(),
    ));
    let mut session = PluginSession::accept(
        tokio::io::stdin(),
        tokio::io::stdout(),
        SessionConfig::default(),
    )
    .await?;
    session.set_call_budgets(CallBudgets::from([(
        Stage::Maintenance,
        domain::ROUND_BUDGET_MS,
    )]))?;
    let plugin = PluginBuilder::from_json(AUTHOR_MANIFEST)?
        .management(service::registration(), {
            let service = service.clone();
            move |call| {
                let service = service.clone();
                async move { service.management(call).await }
            }
        })?
        .on(methods::RECONCILE, move |call| {
            let service = service.clone();
            async move { service.reconcile(call).await }
        })?
        .build()?;
    session.run(plugin).await?;
    Ok(())
}
