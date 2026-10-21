use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use nexus_core::{CommandOutput, CommandRunner};

/// A `CommandRunner` test double: pre-programmed `which` results and
/// canned stdout/stderr/exit-code for exact `(program, args)` pairs,
/// recording every call it receives so tests can assert on invocations.
#[derive(Default)]
pub struct FakeCommandRunner {
    pub which_results: HashMap<String, PathBuf>,
    pub run_results: Mutex<HashMap<(String, Vec<String>), CommandOutput>>,
    pub calls: Mutex<Vec<(String, Vec<String>)>>,
}

impl FakeCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_which(mut self, program: &str, path: &str) -> Self {
        self.which_results
            .insert(program.to_string(), PathBuf::from(path));
        self
    }

    pub fn with_run_result(
        self,
        program: &str,
        args: &[&str],
        stdout: &str,
        stderr: &str,
        status_code: i32,
    ) -> Self {
        let key = (
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        self.run_results.lock().unwrap().insert(
            key,
            CommandOutput {
                status_code: Some(status_code),
                stdout: stdout.as_bytes().to_vec(),
                stderr: stderr.as_bytes().to_vec(),
            },
        );
        self
    }
}

#[async_trait]
impl CommandRunner for FakeCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        _cwd: Option<&Path>,
    ) -> std::io::Result<CommandOutput> {
        self.calls
            .lock()
            .unwrap()
            .push((program.to_string(), args.to_vec()));
        let key = (program.to_string(), args.to_vec());
        self.run_results
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("no fake result programmed for {program} {args:?}"),
                )
            })
    }

    async fn which(&self, program: &str) -> Option<PathBuf> {
        self.which_results.get(program).cloned()
    }
}
