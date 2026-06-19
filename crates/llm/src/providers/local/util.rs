use async_openai::config::OpenAIConfig;

pub fn get_local_config(base_url: &str) -> OpenAIConfig {
    let url = if base_url.ends_with("/v1") { base_url.to_string() } else { format!("{base_url}/v1") };

    OpenAIConfig::new()
        .with_api_key("dummy_key".to_string()) // Local providers generally don't require auth, but async-openai needs a key
        .with_api_base(url)
}
