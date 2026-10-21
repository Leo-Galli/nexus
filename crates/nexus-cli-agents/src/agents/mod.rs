// One submodule per CLI agent. Keep this file to module declarations and
// re-exports only. Windsurf has no officially documented standalone agent
// CLI as of this writing (only unofficial third-party wrappers were
// found) and is therefore NOT implemented here — see
// docs/LIMITATIONS.md. Users can still add it (or any other CLI) as a
// `generic::GenericCliAgent` from Preferences.

pub mod aider;
pub mod antigravity;
pub mod claude_code;
pub mod codex_cli;
pub mod copilot_cli;
pub mod cursor_agent;
pub mod gemini_cli;
pub mod generic;
pub mod qwen_code;

pub use aider::AiderAgent;
pub use antigravity::AntigravityAgent;
pub use claude_code::ClaudeCodeAgent;
pub use codex_cli::CodexCliAgent;
pub use copilot_cli::CopilotCliAgent;
pub use cursor_agent::CursorAgent;
pub use gemini_cli::GeminiCliAgent;
pub use generic::GenericCliAgent;
pub use qwen_code::QwenCodeAgent;
