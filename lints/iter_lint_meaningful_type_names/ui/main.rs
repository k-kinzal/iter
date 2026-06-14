// edition:2024

struct RunnerSummary;
enum TaskOutcome {}
type EmitReport = ();
struct TransitionResult;
struct QueueEngine;
fn construct_runner() {}

struct StatusTransition;
struct AnyAgent;
struct RunCtx;
struct WireContract;

fn staging_dir() {}
fn new() {}

#[path = "auxiliary/config.rs"]
mod config;

fn main() {
    let _ = StatusTransition;
    let _ = AnyAgent;
    let _ = RunCtx;
    let _ = WireContract;
    construct_runner();
    staging_dir();
    new();
    config::layout();
}
