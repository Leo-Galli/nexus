use serde::{Deserialize, Serialize};

use crate::trigger::Trigger;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationDefinition {
    pub name: String,
    pub trigger: Trigger,
    pub steps: Vec<StepConfig>,
}

/// One step in an automation, plus an optional fallback step executed in
/// its place if it fails (e.g. "ask model A, fall back to model B").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepConfig {
    pub id: String,
    pub kind: Step,
    pub on_error: Option<Box<StepConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
    /// Send a prompt to a model. `provider_binding` is looked up in the
    /// `ExecutionContext` the caller supplies, not resolved here.
    Prompt {
        provider_binding: String,
        model_id: String,
        prompt_template: String,
        system_prompt: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        output_variable: String,
    },
    /// Run one non-interactive turn of a CLI agent.
    CliCommand {
        agent_binding: String,
        prompt_template: String,
        working_dir_template: String,
        output_variable: String,
    },
    Transform {
        input_variable: String,
        operation: TransformOp,
        output_variable: String,
    },
    Condition {
        input_variable: String,
        predicate: Predicate,
        then_steps: Vec<StepConfig>,
        else_steps: Vec<StepConfig>,
    },
    Notify {
        title_template: String,
        body_template: String,
    },
    WriteFile {
        path_template: String,
        content_variable: String,
        append: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TransformOp {
    Uppercase,
    Lowercase,
    Trim,
    Replace { from: String, to: String },
    ExtractRegex { pattern: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Predicate {
    Contains { substring: String },
    Equals { value: String },
    MatchesRegex { pattern: String },
    NonEmpty,
}
