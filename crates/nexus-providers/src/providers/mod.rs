// Each submodule implements `nexus_core::Provider` for one service.
// Keep this file to module declarations + re-exports only so independent
// contributors can add a provider without touching anything another
// provider's author is editing.

pub mod anthropic;
pub mod azure_openai;
pub mod bedrock;
pub mod cohere;
pub mod discovery;
pub mod gemini;
pub mod llamacpp;
pub mod lmstudio;
pub mod ollama;
pub mod openai;
pub mod openai_compatible;

pub use anthropic::AnthropicProvider;
pub use azure_openai::AzureOpenAiProvider;
pub use bedrock::BedrockProvider;
pub use cohere::CohereProvider;
pub use gemini::GeminiProvider;
pub use llamacpp::LlamaCppProvider;
pub use lmstudio::LmStudioProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
pub use openai_compatible::OpenAiCompatibleProvider;
