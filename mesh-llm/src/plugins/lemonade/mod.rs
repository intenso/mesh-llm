use anyhow::Result;
use mesh_llm_plugin::{
    plugin_server_info, PluginMetadata, PluginRuntime, PluginStartupPolicy, SimplePlugin,
};

const DEFAULT_LEMONADE_BASE_URL: &str = "http://localhost:8000/api/v1";

fn lemonade_base_url() -> String {
    std::env::var("MESH_LLM_LEMONADE_BASE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_LEMONADE_BASE_URL.to_string())
}

fn lemonade_manifest(base_url: &str) -> mesh_llm_plugin::proto::PluginManifest {
    mesh_llm_plugin::proto::PluginManifest {
        endpoints: vec![mesh_llm_plugin::proto::EndpointManifest {
            endpoint_id: "lemonade".into(),
            kind: mesh_llm_plugin::proto::EndpointKind::Inference as i32,
            transport_kind: mesh_llm_plugin::proto::EndpointTransportKind::EndpointTransportHttp
                as i32,
            protocol: Some("openai_compatible".into()),
            address: Some(base_url.to_string()),
            args: Vec::new(),
            namespace: None,
            supports_streaming: true,
            managed_by_plugin: false,
        }],
        capabilities: vec![
            "endpoint:inference".into(),
            "endpoint:inference/openai_compatible".into(),
        ],
        ..Default::default()
    }
}

fn build_lemonade_plugin(name: String) -> SimplePlugin {
    let base_url = lemonade_base_url();
    let health_url = base_url.clone();

    SimplePlugin::new(
        PluginMetadata::new(
            name,
            crate::VERSION,
            plugin_server_info(
                "mesh-lemonade",
                crate::VERSION,
                "Lemonade Endpoint Plugin",
                "Registers a local Lemonade OpenAI-compatible inference endpoint.",
                Some(
                    "Exposes a local OpenAI-compatible inference endpoint to mesh-llm when enabled.",
                ),
            ),
        )
        .with_capabilities(vec![
            "endpoint:inference".into(),
            "endpoint:inference/openai_compatible".into(),
        ])
        .with_manifest(lemonade_manifest(&base_url))
        .with_startup_policy(PluginStartupPolicy::Any),
    )
    .with_health(move |_context| {
        let health_url = health_url.clone();
        Box::pin(async move { Ok(format!("base_url={health_url}")) })
    })
}

pub(crate) async fn run_plugin(name: String) -> Result<()> {
    PluginRuntime::run(build_lemonade_plugin(name)).await
}
