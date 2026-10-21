use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures_util::future::{BoxFuture, FutureExt};
use futures_util::StreamExt;
use nexus_core::{ChatEvent, ChatMessage, ChatRequest, CliAgent, CommandRunner, Provider};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::error::AutomationError;
use crate::notifier::Notifier;
use crate::step::{AutomationDefinition, Predicate, Step, StepConfig, TransformOp};
use crate::template;

/// Everything the engine needs that the caller (the GTK application) owns:
/// the concrete providers and CLI agents the user has configured, keyed
/// by the binding name an `AutomationDefinition` references, the command
/// runner to execute CLI steps with, and a notification sink.
pub struct ExecutionContext {
    pub providers: HashMap<String, Arc<dyn Provider>>,
    pub cli_agents: HashMap<String, Arc<dyn CliAgent>>,
    pub cli_runner: Arc<dyn CommandRunner>,
    pub notifier: Arc<dyn Notifier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepLogEntry {
    pub step_id: String,
    pub kind: String,
    pub status: String,
    pub detail: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

fn step_kind_name(step: &Step) -> &'static str {
    match step {
        Step::Prompt { .. } => "prompt",
        Step::CliCommand { .. } => "cli_command",
        Step::Transform { .. } => "transform",
        Step::Condition { .. } => "condition",
        Step::Notify { .. } => "notify",
        Step::WriteFile { .. } => "write_file",
    }
}

/// Run every top-level step of `definition` in order, persist a run
/// record (with the full per-step log) through `nexus-store`, and return
/// it. Stops at the first step that fails with no `on_error` fallback (or
/// whose fallback also fails).
pub async fn execute_automation(
    ctx: &ExecutionContext,
    store: &nexus_store::Store,
    automation_id: &str,
    definition: &AutomationDefinition,
) -> Result<nexus_store::models::AutomationRun, AutomationError> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = Utc::now();
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut log: Vec<StepLogEntry> = Vec::new();
    let mut failed = false;

    for step in &definition.steps {
        if execute_step_config(step, &mut vars, ctx, &mut log)
            .await
            .is_err()
        {
            failed = true;
            break;
        }
    }

    let run = nexus_store::models::AutomationRun {
        id: run_id,
        automation_id: automation_id.to_string(),
        started_at,
        finished_at: Some(Utc::now()),
        status: if failed { "failed" } else { "success" }.to_string(),
        log_json: serde_json::to_string(&log)?,
    };
    nexus_store::automations::record_run(store, run.clone()).await?;
    Ok(run)
}

/// Execute one step, logging its outcome, and — on failure — falling
/// back to `step.on_error` (recursively) if one is configured. Boxed
/// because `Step::Condition` branches recurse back into this function.
fn execute_step_config<'a>(
    step: &'a StepConfig,
    vars: &'a mut HashMap<String, String>,
    ctx: &'a ExecutionContext,
    log: &'a mut Vec<StepLogEntry>,
) -> BoxFuture<'a, Result<(), AutomationError>> {
    async move {
        let started_at = Utc::now();
        let result = execute_step_kind(&step.kind, vars, ctx, log).await;
        match result {
            Ok(()) => {
                log.push(StepLogEntry {
                    step_id: step.id.clone(),
                    kind: step_kind_name(&step.kind).to_string(),
                    status: "success".to_string(),
                    detail: None,
                    started_at,
                    finished_at: Utc::now(),
                });
                Ok(())
            }
            Err(err) => {
                log.push(StepLogEntry {
                    step_id: step.id.clone(),
                    kind: step_kind_name(&step.kind).to_string(),
                    status: "failed".to_string(),
                    detail: Some(err.to_string()),
                    started_at,
                    finished_at: Utc::now(),
                });
                match &step.on_error {
                    Some(fallback) => execute_step_config(fallback, vars, ctx, log).await,
                    None => Err(err),
                }
            }
        }
    }
    .boxed()
}

async fn execute_step_kind(
    kind: &Step,
    vars: &mut HashMap<String, String>,
    ctx: &ExecutionContext,
    log: &mut Vec<StepLogEntry>,
) -> Result<(), AutomationError> {
    match kind {
        Step::Prompt {
            provider_binding,
            model_id,
            prompt_template,
            system_prompt,
            temperature,
            max_tokens,
            output_variable,
        } => {
            let provider = ctx
                .providers
                .get(provider_binding)
                .ok_or_else(|| AutomationError::UnknownProvider(provider_binding.clone()))?;
            let prompt = template::render(prompt_template, vars)?;
            let request = ChatRequest {
                model_id: model_id.clone(),
                messages: vec![ChatMessage::user(prompt)],
                system_prompt: system_prompt.clone(),
                temperature: *temperature,
                max_tokens: *max_tokens,
                tools: Vec::new(),
                stream: true,
            };
            let mut stream = provider.send_chat(request).await?;
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                match event? {
                    ChatEvent::TextDelta(delta) => text.push_str(&delta),
                    ChatEvent::Done { .. } => break,
                    ChatEvent::Usage(_) | ChatEvent::ToolCallDelta { .. } => {}
                }
            }
            vars.insert(output_variable.clone(), text);
            Ok(())
        }
        Step::CliCommand {
            agent_binding,
            prompt_template,
            working_dir_template,
            output_variable,
        } => {
            let agent = ctx
                .cli_agents
                .get(agent_binding)
                .ok_or_else(|| AutomationError::UnknownCliAgent(agent_binding.clone()))?;
            let prompt = template::render(prompt_template, vars)?;
            let working_dir = template::render(working_dir_template, vars)?;
            let cwd = PathBuf::from(working_dir);
            let invocation = agent
                .build_noninteractive_command(&prompt, &cwd, false)
                .map_err(|e| AutomationError::CliUnsupported(e.to_string()))?;
            let output = ctx
                .cli_runner
                .run(&invocation.program, &invocation.args, invocation.cwd.as_deref())
                .await?;
            if !output.success() {
                return Err(AutomationError::CliFailed(format!(
                    "exit {:?}: {}",
                    output.status_code,
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            vars.insert(output_variable.clone(), output.stdout_utf8_lossy().into_owned());
            Ok(())
        }
        Step::Transform {
            input_variable,
            operation,
            output_variable,
        } => {
            let input = vars
                .get(input_variable)
                .ok_or_else(|| AutomationError::UnknownVariable(input_variable.clone()))?
                .clone();
            let output = match operation {
                TransformOp::Uppercase => input.to_uppercase(),
                TransformOp::Lowercase => input.to_lowercase(),
                TransformOp::Trim => input.trim().to_string(),
                TransformOp::Replace { from, to } => input.replace(from.as_str(), to),
                TransformOp::ExtractRegex { pattern } => {
                    let re = Regex::new(pattern)
                        .map_err(|e| AutomationError::InvalidRegex(e.to_string()))?;
                    re.find(&input).map(|m| m.as_str().to_string()).unwrap_or_default()
                }
            };
            vars.insert(output_variable.clone(), output);
            Ok(())
        }
        Step::Condition {
            input_variable,
            predicate,
            then_steps,
            else_steps,
        } => {
            let input = vars
                .get(input_variable)
                .ok_or_else(|| AutomationError::UnknownVariable(input_variable.clone()))?
                .clone();
            let matched = match predicate {
                Predicate::Contains { substring } => input.contains(substring.as_str()),
                Predicate::Equals { value } => &input == value,
                Predicate::MatchesRegex { pattern } => Regex::new(pattern)
                    .map_err(|e| AutomationError::InvalidRegex(e.to_string()))?
                    .is_match(&input),
                Predicate::NonEmpty => !input.trim().is_empty(),
            };
            let branch = if matched { then_steps } else { else_steps };
            for step in branch {
                execute_step_config(step, vars, ctx, log).await?;
            }
            Ok(())
        }
        Step::Notify {
            title_template,
            body_template,
        } => {
            let title = template::render(title_template, vars)?;
            let body = template::render(body_template, vars)?;
            ctx.notifier.notify(&title, &body);
            Ok(())
        }
        Step::WriteFile {
            path_template,
            content_variable,
            append,
        } => {
            let path = template::render(path_template, vars)?;
            let content = vars
                .get(content_variable)
                .ok_or_else(|| AutomationError::UnknownVariable(content_variable.clone()))?
                .clone();
            let mut file = if *append {
                tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .await?
            } else {
                tokio::fs::File::create(&path).await?
            };
            file.write_all(content.as_bytes()).await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Trigger;
    use async_trait::async_trait;
    use futures_util::stream::BoxStream;
    use nexus_core::{
        AuthMethod, ChatStream, FinishReason, ModelInfo, ProviderError, ProviderKind,
        ProviderResult, ProviderStatus,
    };

    struct FixedProvider {
        reply: &'static str,
        fail: bool,
    }

    #[async_trait]
    impl Provider for FixedProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::OpenAiCompatibleCustom
        }
        fn display_name(&self) -> &'static str {
            "fixture"
        }
        fn auth_method(&self) -> AuthMethod {
            AuthMethod::None
        }
        fn api_key_url(&self) -> Option<&'static str> {
            None
        }
        async fn status(&self) -> ProviderResult<ProviderStatus> {
            Ok(ProviderStatus::not_configured())
        }
        async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
            Ok(vec![])
        }
        async fn send_chat(&self, _request: ChatRequest) -> ProviderResult<ChatStream> {
            if self.fail {
                return Err(ProviderError::NotConfigured);
            }
            let events: Vec<ProviderResult<ChatEvent>> = vec![
                Ok(ChatEvent::TextDelta(self.reply.to_string())),
                Ok(ChatEvent::Done {
                    finish_reason: FinishReason::Stop,
                }),
            ];
            let stream: BoxStream<'static, ProviderResult<ChatEvent>> =
                Box::pin(futures_util::stream::iter(events));
            Ok(stream)
        }
    }

    fn ctx_with(providers: HashMap<String, Arc<dyn Provider>>) -> ExecutionContext {
        ExecutionContext {
            providers,
            cli_agents: HashMap::new(),
            cli_runner: Arc::new(tests_support::NoopRunner),
            notifier: Arc::new(crate::notifier::NullNotifier),
        }
    }

    /// `automation_runs.automation_id` has a foreign key into
    /// `automations.id`, so tests driving `execute_automation` against a
    /// real `Store` must first create the parent automation row.
    async fn seed_automation(store: &nexus_store::Store, id: &str) {
        let now = Utc::now();
        nexus_store::automations::upsert(
            store,
            nexus_store::models::Automation {
                id: id.to_string(),
                name: "test automation".to_string(),
                definition_json: "{}".to_string(),
                enabled: true,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn prompt_step_fills_variable() {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            "main".into(),
            Arc::new(FixedProvider {
                reply: "hello",
                fail: false,
            }),
        );
        let ctx = ctx_with(providers);
        let store = nexus_store::Store::open_in_memory().await.unwrap();
        seed_automation(&store, "auto-1").await;

        let definition = AutomationDefinition {
            name: "t".into(),
            trigger: Trigger::Manual,
            steps: vec![StepConfig {
                id: "s1".into(),
                kind: Step::Prompt {
                    provider_binding: "main".into(),
                    model_id: "m".into(),
                    prompt_template: "hi".into(),
                    system_prompt: None,
                    temperature: None,
                    max_tokens: None,
                    output_variable: "out".into(),
                },
                on_error: None,
            }],
        };
        let run = execute_automation(&ctx, &store, "auto-1", &definition)
            .await
            .unwrap();
        assert_eq!(run.status, "success");
        let log: Vec<StepLogEntry> = serde_json::from_str(&run.log_json).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].status, "success");
    }

    #[tokio::test]
    async fn fallback_runs_when_primary_fails() {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            "broken".into(),
            Arc::new(FixedProvider {
                reply: "",
                fail: true,
            }),
        );
        providers.insert(
            "backup".into(),
            Arc::new(FixedProvider {
                reply: "fallback reply",
                fail: false,
            }),
        );
        let ctx = ctx_with(providers);
        let store = nexus_store::Store::open_in_memory().await.unwrap();
        seed_automation(&store, "auto-1").await;

        let fallback_step = StepConfig {
            id: "s1-fallback".into(),
            kind: Step::Prompt {
                provider_binding: "backup".into(),
                model_id: "m".into(),
                prompt_template: "hi".into(),
                system_prompt: None,
                temperature: None,
                max_tokens: None,
                output_variable: "out".into(),
            },
            on_error: None,
        };
        let definition = AutomationDefinition {
            name: "t".into(),
            trigger: Trigger::Manual,
            steps: vec![StepConfig {
                id: "s1".into(),
                kind: Step::Prompt {
                    provider_binding: "broken".into(),
                    model_id: "m".into(),
                    prompt_template: "hi".into(),
                    system_prompt: None,
                    temperature: None,
                    max_tokens: None,
                    output_variable: "out".into(),
                },
                on_error: Some(Box::new(fallback_step)),
            }],
        };
        let run = execute_automation(&ctx, &store, "auto-1", &definition)
            .await
            .unwrap();
        assert_eq!(run.status, "success");
        let log: Vec<StepLogEntry> = serde_json::from_str(&run.log_json).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].status, "failed");
        assert_eq!(log[1].status, "success");
    }

    #[tokio::test]
    async fn condition_step_picks_matching_branch_only() {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            "main".into(),
            Arc::new(FixedProvider {
                reply: "yes-match-xyz",
                fail: false,
            }),
        );
        let ctx = ctx_with(providers);
        let store = nexus_store::Store::open_in_memory().await.unwrap();
        seed_automation(&store, "auto-1").await;

        let then_step = StepConfig {
            id: "then-notify".into(),
            kind: Step::Notify {
                title_template: "matched".into(),
                body_template: "matched".into(),
            },
            on_error: None,
        };
        let else_step = StepConfig {
            id: "else-notify".into(),
            kind: Step::Notify {
                title_template: "not matched".into(),
                body_template: "not matched".into(),
            },
            on_error: None,
        };
        let definition = AutomationDefinition {
            name: "t".into(),
            trigger: Trigger::Manual,
            steps: vec![
                StepConfig {
                    id: "ask".into(),
                    kind: Step::Prompt {
                        provider_binding: "main".into(),
                        model_id: "m".into(),
                        prompt_template: "hi".into(),
                        system_prompt: None,
                        temperature: None,
                        max_tokens: None,
                        output_variable: "result".into(),
                    },
                    on_error: None,
                },
                StepConfig {
                    id: "check".into(),
                    kind: Step::Condition {
                        input_variable: "result".into(),
                        predicate: Predicate::Contains {
                            substring: "match".into(),
                        },
                        then_steps: vec![then_step],
                        else_steps: vec![else_step],
                    },
                    on_error: None,
                },
            ],
        };
        let run = execute_automation(&ctx, &store, "auto-1", &definition)
            .await
            .unwrap();
        assert_eq!(run.status, "success");
        let log: Vec<StepLogEntry> = serde_json::from_str(&run.log_json).unwrap();
        let step_ids: Vec<&str> = log.iter().map(|e| e.step_id.as_str()).collect();
        assert!(step_ids.contains(&"then-notify"));
        assert!(!step_ids.contains(&"else-notify"));
    }

    mod tests_support {
        use async_trait::async_trait;
        use nexus_core::{CommandOutput, CommandRunner};
        use std::path::{Path, PathBuf};

        pub struct NoopRunner;

        #[async_trait]
        impl CommandRunner for NoopRunner {
            async fn run(
                &self,
                _program: &str,
                _args: &[String],
                _cwd: Option<&Path>,
            ) -> std::io::Result<CommandOutput> {
                Ok(CommandOutput {
                    status_code: Some(0),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
            }
            async fn which(&self, _program: &str) -> Option<PathBuf> {
                None
            }
        }
    }
}
