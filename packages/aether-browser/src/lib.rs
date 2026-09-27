#![doc = include_str!("../README.md")]

pub mod types;

mod client;
mod connection;
mod conversation;
mod elicitation;
mod error;
mod js;
mod websocket;

pub use client::AetherClient;
pub use elicitation::Elicitation;
pub use error::ClientError;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TYPESCRIPT: &str = r#"
import type {
  CloseSessionResponse,
  ContentBlock,
  CreateElicitationRequest,
  CreateElicitationResponse,
  InitializeResponse,
  NewSessionRequest,
  NewSessionResponse,
  PromptResponse,
  ResumeSessionRequest,
  ResumeSessionResponse,
} from "@agentclientprotocol/sdk/experimental/v2";
import type {
  AetherClientErrorDetails,
  AetherClientEvent,
  AetherClientOptions,
  RemoteServerInfo,
} from "./types.js";

export type * from "./types.js";

/** The error every method of `AetherClient` and `Elicitation` throws or rejects with. */
export interface AetherClientError extends Error, AetherClientErrorDetails {}
"#;
