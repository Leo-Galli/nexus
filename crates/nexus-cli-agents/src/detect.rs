use nexus_core::{CliAgent, CliAgentInfo, CliDetectionResult, CommandRunner};

/// Detect every given agent concurrently against the same runner. Used at
/// application startup and from the "Refresh" action in Preferences.
pub async fn detect_all<'a>(
    runner: &dyn CommandRunner,
    agents: &[&'a dyn CliAgent],
) -> Vec<(CliAgentInfo, CliDetectionResult)> {
    let mut results = Vec::with_capacity(agents.len());
    // Sequential await over a trait-object slice keeps this free of extra
    // dependencies (no need for FuturesUnordered); detection is cheap
    // (one or two short-lived subprocesses per agent) so this is fine for
    // the handful of agents Nexus ships.
    for agent in agents {
        let detection = agent.detect(runner).await;
        results.push((agent.info(), detection));
    }
    results
}
