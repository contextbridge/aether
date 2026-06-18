#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Text,
    Pretty,
    Json,
}
