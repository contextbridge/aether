pub(crate) mod html_review;
mod server;
pub mod tools;

pub use server::{ReviewMcp, ReviewMcpArgs};
pub use tools::{DocumentSource, HtmlAnnotation, HtmlSource, ReviewArtifactInput, ReviewArtifactOutput};
