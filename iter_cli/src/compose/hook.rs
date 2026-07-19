//! Compose Hook plan construction and orchestrator-side dispatch.
//!
//! These hooks observe only state owned by the Compose orchestrator: the
//! Compose run itself, managed iter processes, and managed Trigger
//! supervisors. They never inspect Runner lifecycle or iteration state.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::Local;
use iter_core::{Template, TemplateError};
use iter_language::{Action, ComposeEventName, ComposeHookDef, Spanned};
use serde::Serialize;
use thiserror::Error;
use tracing::warn;

use super::supervisor::TriggerLifecycleState;
use crate::shell_action::run_shell_command;

/// A service terminal state visible to the Compose orchestrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceTerminalState {
    /// The iter process reached `ProcessStatus::Stopped`.
    Completed,
    /// The iter process failed to start, run, monitor, or finalize.
    Failed,
    /// The iter process reached `ProcessStatus::Killed`.
    Killed,
}

impl ServiceTerminalState {
    fn status_str(self) -> &'static str {
        match self {
            Self::Completed => "stopped",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }

    fn event(self) -> ComposeEventName {
        match self {
            Self::Completed => ComposeEventName::ServiceCompleted,
            Self::Failed => ComposeEventName::ServiceFailed,
            Self::Killed => ComposeEventName::ServiceKilled,
        }
    }
}

/// Intermediate lifecycle notifications emitted by managed tasks.
#[derive(Debug)]
pub(crate) enum ManagedEvent {
    /// An iter process reached `Running`.
    ServiceStarted {
        /// Compose service name.
        name: String,
    },
    /// A Trigger supervisor changed observable lifecycle state.
    TriggerTransition {
        /// Compose Trigger name.
        name: String,
        /// New supervisor state.
        state: TriggerLifecycleState,
        /// Number of supervisor restarts so far.
        restart_count: u32,
        /// Most recent failure detail, when present.
        error: Option<String>,
    },
}

/// Failure while converting language-level hooks into a runnable plan.
#[derive(Debug, Error)]
pub(crate) enum ComposeHookBuildError {
    /// A handler selected a service absent from the flattened plan.
    #[error("Compose hook `{event}` references unknown service `{name}`")]
    UnknownService {
        /// Event spelling.
        event: &'static str,
        /// Missing service name.
        name: String,
    },
    /// A handler selected a Trigger absent from the flattened plan.
    #[error("Compose hook `{event}` references unknown trigger `{name}`")]
    UnknownTrigger {
        /// Event spelling.
        event: &'static str,
        /// Missing Trigger name.
        name: String,
    },
    /// A shell command could not be compiled as a strict template.
    #[error("invalid shell template in Compose hook `{event}`: {source}")]
    Template {
        /// Event spelling.
        event: &'static str,
        /// Template compiler error.
        #[source]
        source: TemplateError,
    },
}

#[derive(Debug, Clone)]
struct ComposeShellAction {
    source: String,
    template: Template,
}

#[derive(Debug, Clone)]
pub(crate) struct ComposeHookPlan {
    event: ComposeEventName,
    services: Vec<String>,
    triggers: Vec<String>,
    actions: Vec<ComposeShellAction>,
}

/// Resolve selectors against the flattened resource set and compile actions.
pub(crate) fn build_hook_plans(
    declarations: &[Spanned<ComposeHookDef>],
    service_names: &[String],
    trigger_names: &[String],
) -> Result<Vec<ComposeHookPlan>, ComposeHookBuildError> {
    let known_services: BTreeSet<&str> = service_names.iter().map(String::as_str).collect();
    let known_triggers: BTreeSet<&str> = trigger_names.iter().map(String::as_str).collect();
    let mut plans = Vec::with_capacity(declarations.len());

    for declaration in declarations {
        let hook = &declaration.node;
        let services = if hook.event.uses_services() {
            let selected = hook
                .services
                .clone()
                .unwrap_or_else(|| service_names.to_vec());
            for name in &selected {
                if !known_services.contains(name.as_str()) {
                    return Err(ComposeHookBuildError::UnknownService {
                        event: hook.event.as_str(),
                        name: name.clone(),
                    });
                }
            }
            selected
        } else {
            Vec::new()
        };
        let triggers = if hook.event.uses_triggers() {
            let selected = hook
                .triggers
                .clone()
                .unwrap_or_else(|| trigger_names.to_vec());
            for name in &selected {
                if !known_triggers.contains(name.as_str()) {
                    return Err(ComposeHookBuildError::UnknownTrigger {
                        event: hook.event.as_str(),
                        name: name.clone(),
                    });
                }
            }
            selected
        } else {
            Vec::new()
        };

        let mut actions = Vec::with_capacity(hook.actions.len());
        for action in &hook.actions {
            match action {
                Action::Shell(source) => {
                    let template = Template::compile(source.clone()).map_err(|source| {
                        ComposeHookBuildError::Template {
                            event: hook.event.as_str(),
                            source,
                        }
                    })?;
                    actions.push(ComposeShellAction {
                        source: source.clone(),
                        template,
                    });
                }
            }
        }
        plans.push(ComposeHookPlan {
            event: hook.event,
            services,
            triggers,
            actions,
        });
    }
    Ok(plans)
}

#[derive(Debug)]
struct HookState {
    plan: ComposeHookPlan,
    fired: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceState {
    Initializing,
    Running,
    Terminal(ServiceTerminalState),
}

/// Stateful dispatcher owned by one Compose orchestrator run.
#[derive(Debug)]
pub(crate) struct ComposeHookRuntime {
    hooks: Vec<HookState>,
    project: String,
    compose_file: PathBuf,
    cwd: PathBuf,
    services: BTreeMap<String, ServiceState>,
    services_started: BTreeSet<String>,
    triggers: BTreeMap<String, TriggerLifecycleState>,
    triggers_started: BTreeSet<String>,
}

impl ComposeHookRuntime {
    /// Instantiate run-local state from a completed plan.
    #[must_use]
    pub(crate) fn new(
        plans: Vec<ComposeHookPlan>,
        project: String,
        compose_file: PathBuf,
        service_names: Vec<String>,
        trigger_names: Vec<String>,
    ) -> Self {
        let cwd = compose_file
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self {
            hooks: plans
                .into_iter()
                .map(|plan| HookState { plan, fired: false })
                .collect(),
            project,
            compose_file,
            cwd,
            services: service_names
                .into_iter()
                .map(|name| (name, ServiceState::Initializing))
                .collect(),
            services_started: BTreeSet::new(),
            triggers: trigger_names
                .into_iter()
                .map(|name| (name, TriggerLifecycleState::Starting))
                .collect(),
            triggers_started: BTreeSet::new(),
        }
    }

    /// Dispatch a Compose-run event.
    pub(crate) async fn compose_event(&mut self, event: ComposeEventName, error: Option<String>) {
        let context = self.context(event, None, None, &[], &[], error);
        self.dispatch_direct(event, context).await;
    }

    /// Dispatch `service_starting`.
    pub(crate) async fn service_starting(&mut self, name: &str) {
        let context = self.context(
            ComposeEventName::ServiceStarting,
            Some((name, "initializing")),
            None,
            &[],
            &[],
            None,
        );
        self.dispatch_resource(ComposeEventName::ServiceStarting, name, context)
            .await;
    }

    /// Record and dispatch `service_started`, then evaluate service aggregates.
    pub(crate) async fn service_started(&mut self, name: &str) {
        self.services.insert(name.to_owned(), ServiceState::Running);
        self.services_started.insert(name.to_owned());
        let context = self.context(
            ComposeEventName::ServiceStarted,
            Some((name, "running")),
            None,
            &[],
            &[],
            None,
        );
        self.dispatch_resource(ComposeEventName::ServiceStarted, name, context)
            .await;
        self.evaluate_service_aggregates(Some((name, "running")), None)
            .await;
    }

    /// Record a terminal service state, dispatch its event, and evaluate aggregates.
    pub(crate) async fn service_terminal(
        &mut self,
        name: &str,
        state: ServiceTerminalState,
        error: Option<String>,
    ) {
        self.services
            .insert(name.to_owned(), ServiceState::Terminal(state));
        let event = state.event();
        let context = self.context(
            event,
            Some((name, state.status_str())),
            None,
            &[],
            &[],
            error.clone(),
        );
        self.dispatch_resource(event, name, context).await;
        self.evaluate_service_aggregates(Some((name, state.status_str())), error)
            .await;
    }

    /// Dispatch `trigger_starting`.
    pub(crate) async fn trigger_starting(&mut self, name: &str) {
        let context = self.context(
            ComposeEventName::TriggerStarting,
            None,
            Some((name, "starting", 0)),
            &[],
            &[],
            None,
        );
        self.dispatch_resource(ComposeEventName::TriggerStarting, name, context)
            .await;
    }

    /// Record and dispatch a Trigger supervisor transition.
    pub(crate) async fn trigger_transition(
        &mut self,
        name: &str,
        state: TriggerLifecycleState,
        restart_count: u32,
        error: Option<String>,
    ) {
        self.triggers.insert(name.to_owned(), state);
        let event = match state {
            TriggerLifecycleState::Starting => return,
            TriggerLifecycleState::Running => {
                self.triggers_started.insert(name.to_owned());
                ComposeEventName::TriggerStarted
            }
            TriggerLifecycleState::Restarting => ComposeEventName::TriggerRestarting,
            TriggerLifecycleState::Completed => ComposeEventName::TriggerCompleted,
            TriggerLifecycleState::Failed => ComposeEventName::TriggerFailed,
            TriggerLifecycleState::Stopped => ComposeEventName::TriggerStopped,
        };
        let context = self.context(
            event,
            None,
            Some((name, trigger_state_str(state), restart_count)),
            &[],
            &[],
            error.clone(),
        );
        self.dispatch_resource(event, name, context).await;
        self.evaluate_trigger_aggregates(
            Some((name, trigger_state_str(state), restart_count)),
            error,
        )
        .await;
    }

    /// Whether every declared resource reached `Running` at least once.
    #[must_use]
    pub(crate) fn all_resources_started(&self) -> bool {
        self.services_started.len() == self.services.len()
            && self.triggers_started.len() == self.triggers.len()
    }

    async fn dispatch_direct(&mut self, event: ComposeEventName, context: HookContext) {
        let indices: Vec<usize> = self
            .hooks
            .iter()
            .enumerate()
            .filter_map(|(index, hook)| (hook.plan.event == event).then_some(index))
            .collect();
        for index in indices {
            self.run_hook(index, &context).await;
        }
    }

    async fn dispatch_resource(
        &mut self,
        event: ComposeEventName,
        name: &str,
        context: HookContext,
    ) {
        let indices: Vec<usize> = self
            .hooks
            .iter()
            .enumerate()
            .filter_map(|(index, hook)| {
                let selected = if event.uses_services() {
                    &hook.plan.services
                } else {
                    &hook.plan.triggers
                };
                (hook.plan.event == event && selected.iter().any(|item| item == name))
                    .then_some(index)
            })
            .collect();
        for index in indices {
            self.run_hook(index, &context).await;
        }
    }

    async fn evaluate_service_aggregates(
        &mut self,
        cause: Option<(&str, &str)>,
        error: Option<String>,
    ) {
        let indices: Vec<usize> = self
            .hooks
            .iter()
            .enumerate()
            .filter_map(|(index, hook)| {
                (!hook.fired
                    && hook.plan.event.is_aggregate()
                    && hook.plan.event.uses_services()
                    && self.service_aggregate_satisfied(&hook.plan))
                .then_some(index)
            })
            .collect();
        for index in indices {
            self.hooks[index].fired = true;
            let names = self.hooks[index].plan.services.clone();
            let context = self.context(
                self.hooks[index].plan.event,
                cause,
                None,
                &names,
                &[],
                error.clone(),
            );
            self.run_hook(index, &context).await;
        }
    }

    async fn evaluate_trigger_aggregates(
        &mut self,
        cause: Option<(&str, &str, u32)>,
        error: Option<String>,
    ) {
        let indices: Vec<usize> = self
            .hooks
            .iter()
            .enumerate()
            .filter_map(|(index, hook)| {
                (!hook.fired
                    && hook.plan.event.is_aggregate()
                    && hook.plan.event.uses_triggers()
                    && self.trigger_aggregate_satisfied(&hook.plan))
                .then_some(index)
            })
            .collect();
        for index in indices {
            self.hooks[index].fired = true;
            let names = self.hooks[index].plan.triggers.clone();
            let context = self.context(
                self.hooks[index].plan.event,
                None,
                cause,
                &[],
                &names,
                error.clone(),
            );
            self.run_hook(index, &context).await;
        }
    }

    fn service_aggregate_satisfied(&self, plan: &ComposeHookPlan) -> bool {
        match plan.event {
            ComposeEventName::ServicesStarted => plan
                .services
                .iter()
                .all(|name| self.services_started.contains(name)),
            ComposeEventName::ServicesCompleted => plan.services.iter().all(|name| {
                matches!(
                    self.services.get(name),
                    Some(ServiceState::Terminal(ServiceTerminalState::Completed))
                )
            }),
            ComposeEventName::ServicesFailed => plan.services.iter().any(|name| {
                matches!(
                    self.services.get(name),
                    Some(ServiceState::Terminal(ServiceTerminalState::Failed))
                )
            }),
            ComposeEventName::ServicesSettled => plan
                .services
                .iter()
                .all(|name| matches!(self.services.get(name), Some(ServiceState::Terminal(_)))),
            _ => false,
        }
    }

    fn trigger_aggregate_satisfied(&self, plan: &ComposeHookPlan) -> bool {
        match plan.event {
            ComposeEventName::TriggersStarted => plan
                .triggers
                .iter()
                .all(|name| self.triggers_started.contains(name)),
            ComposeEventName::TriggersCompleted => plan
                .triggers
                .iter()
                .all(|name| self.triggers.get(name) == Some(&TriggerLifecycleState::Completed)),
            ComposeEventName::TriggersFailed => plan
                .triggers
                .iter()
                .any(|name| self.triggers.get(name) == Some(&TriggerLifecycleState::Failed)),
            ComposeEventName::TriggersSettled => plan.triggers.iter().all(|name| {
                matches!(
                    self.triggers.get(name),
                    Some(
                        TriggerLifecycleState::Completed
                            | TriggerLifecycleState::Failed
                            | TriggerLifecycleState::Stopped
                    )
                )
            }),
            _ => false,
        }
    }

    async fn run_hook(&self, index: usize, context: &HookContext) {
        let hook = &self.hooks[index].plan;
        let env = context.environment();
        for (action_index, action) in hook.actions.iter().enumerate() {
            let rendered = match action.template.render(context) {
                Ok(rendered) => rendered,
                Err(error) => {
                    warn!(
                        event = hook.event.as_str(),
                        action_index,
                        command = %action.source,
                        error = %error,
                        "Compose Hook template render failed",
                    );
                    continue;
                }
            };
            if let Err(error) = run_shell_command(&rendered, Some(&self.cwd), &env).await {
                warn!(
                    event = hook.event.as_str(),
                    action_index,
                    command = %rendered,
                    error = %error,
                    "Compose Hook shell action failed to start",
                );
            }
        }
    }

    fn context(
        &self,
        event: ComposeEventName,
        service: Option<(&str, &str)>,
        trigger: Option<(&str, &str, u32)>,
        services: &[String],
        triggers: &[String],
        error: Option<String>,
    ) -> HookContext {
        HookContext {
            today: Local::now().date_naive().format("%Y-%m-%d").to_string(),
            event: EventView {
                name: event.as_str(),
            },
            compose: ComposeView {
                project: self.project.clone(),
                file: self.compose_file.display().to_string(),
            },
            service: service.map(|(name, status)| ResourceView {
                name: name.to_owned(),
                status: status.to_owned(),
            }),
            trigger: trigger.map(|(name, status, restart_count)| TriggerView {
                name: name.to_owned(),
                status: status.to_owned(),
                restart_count,
            }),
            services: (!services.is_empty()).then(|| AggregateView {
                names: services.join(","),
                count: services.len(),
            }),
            triggers: (!triggers.is_empty()).then(|| AggregateView {
                names: triggers.join(","),
                count: triggers.len(),
            }),
            error: error.map(|message| ErrorView { message }),
        }
    }
}

fn trigger_state_str(state: TriggerLifecycleState) -> &'static str {
    match state {
        TriggerLifecycleState::Starting => "starting",
        TriggerLifecycleState::Running => "running",
        TriggerLifecycleState::Restarting => "restarting",
        TriggerLifecycleState::Failed => "failed",
        TriggerLifecycleState::Stopped => "stopped",
        TriggerLifecycleState::Completed => "completed",
    }
}

#[derive(Debug, Serialize)]
struct HookContext {
    today: String,
    event: EventView,
    compose: ComposeView,
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<ResourceView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<TriggerView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    services: Option<AggregateView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    triggers: Option<AggregateView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorView>,
}

impl HookContext {
    fn environment(&self) -> Vec<(String, String)> {
        let mut env: BTreeMap<String, String> = [
            "ITER_COMPOSE_EVENT",
            "ITER_COMPOSE_PROJECT",
            "ITER_COMPOSE_FILE",
            "ITER_COMPOSE_SERVICE",
            "ITER_COMPOSE_SERVICE_STATUS",
            "ITER_COMPOSE_TRIGGER",
            "ITER_COMPOSE_TRIGGER_STATUS",
            "ITER_COMPOSE_TRIGGER_RESTART_COUNT",
            "ITER_COMPOSE_SERVICES",
            "ITER_COMPOSE_TRIGGERS",
            "ITER_COMPOSE_ERROR",
        ]
        .into_iter()
        .map(|name| (name.to_owned(), String::new()))
        .collect();
        env.insert("ITER_COMPOSE_EVENT".to_owned(), self.event.name.to_owned());
        env.insert(
            "ITER_COMPOSE_PROJECT".to_owned(),
            self.compose.project.clone(),
        );
        env.insert("ITER_COMPOSE_FILE".to_owned(), self.compose.file.clone());
        if let Some(service) = &self.service {
            env.insert("ITER_COMPOSE_SERVICE".to_owned(), service.name.clone());
            env.insert(
                "ITER_COMPOSE_SERVICE_STATUS".to_owned(),
                service.status.clone(),
            );
        }
        if let Some(trigger) = &self.trigger {
            env.insert("ITER_COMPOSE_TRIGGER".to_owned(), trigger.name.clone());
            env.insert(
                "ITER_COMPOSE_TRIGGER_STATUS".to_owned(),
                trigger.status.clone(),
            );
            env.insert(
                "ITER_COMPOSE_TRIGGER_RESTART_COUNT".to_owned(),
                trigger.restart_count.to_string(),
            );
        }
        if let Some(services) = &self.services {
            env.insert("ITER_COMPOSE_SERVICES".to_owned(), services.names.clone());
        }
        if let Some(triggers) = &self.triggers {
            env.insert("ITER_COMPOSE_TRIGGERS".to_owned(), triggers.names.clone());
        }
        if let Some(error) = &self.error {
            env.insert("ITER_COMPOSE_ERROR".to_owned(), error.message.clone());
        }
        env.into_iter().collect()
    }
}

#[derive(Debug, Serialize)]
struct EventView {
    name: &'static str,
}

#[derive(Debug, Serialize)]
struct ComposeView {
    project: String,
    file: String,
}

#[derive(Debug, Serialize)]
struct ResourceView {
    name: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct TriggerView {
    name: String,
    status: String,
    restart_count: u32,
}

#[derive(Debug, Serialize)]
struct AggregateView {
    names: String,
    count: usize,
}

#[derive(Debug, Serialize)]
struct ErrorView {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration(
        event: ComposeEventName,
        services: Option<Vec<&str>>,
        triggers: Option<Vec<&str>>,
        command: &str,
    ) -> Spanned<ComposeHookDef> {
        Spanned::new(
            ComposeHookDef {
                event,
                services: services.map(|names| names.into_iter().map(str::to_owned).collect()),
                triggers: triggers.map(|names| names.into_iter().map(str::to_owned).collect()),
                actions: vec![Action::Shell(command.to_owned())],
            },
            0..0,
        )
    }

    fn runtime(
        directory: &Path,
        declarations: Vec<Spanned<ComposeHookDef>>,
        services: &[&str],
        triggers: &[&str],
    ) -> ComposeHookRuntime {
        let service_names: Vec<String> = services.iter().map(|name| (*name).to_owned()).collect();
        let trigger_names: Vec<String> = triggers.iter().map(|name| (*name).to_owned()).collect();
        let plans = build_hook_plans(&declarations, &service_names, &trigger_names)
            .expect("build hook plans");
        ComposeHookRuntime::new(
            plans,
            "demo".to_owned(),
            directory.join("compose.iter"),
            service_names,
            trigger_names,
        )
    }

    #[tokio::test]
    async fn services_completed_waits_for_selected_set_and_fires_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let declarations = vec![declaration(
            ComposeEventName::ServicesCompleted,
            Some(vec!["a", "b"]),
            None,
            "echo '{{event.name}} {{services.names}}' > completed.txt",
        )];
        let mut hooks = runtime(directory.path(), declarations, &["a", "b", "other"], &[]);

        hooks
            .service_terminal("a", ServiceTerminalState::Completed, None)
            .await;
        assert!(!directory.path().join("completed.txt").exists());

        hooks
            .service_terminal("b", ServiceTerminalState::Completed, None)
            .await;
        let contents =
            std::fs::read_to_string(directory.path().join("completed.txt")).expect("marker");
        assert_eq!(contents.trim(), "services_completed a,b");

        hooks
            .service_terminal("other", ServiceTerminalState::Completed, None)
            .await;
        let contents_after =
            std::fs::read_to_string(directory.path().join("completed.txt")).expect("marker");
        assert_eq!(contents_after, contents);
    }

    #[tokio::test]
    async fn services_failed_uses_first_selected_failure_only() {
        let directory = tempfile::tempdir().expect("tempdir");
        let declarations = vec![declaration(
            ComposeEventName::ServicesFailed,
            None,
            None,
            "echo \"$ITER_COMPOSE_SERVICE:$ITER_COMPOSE_ERROR\" >> failures.txt",
        )];
        let mut hooks = runtime(directory.path(), declarations, &["a", "b"], &[]);

        hooks
            .service_terminal("a", ServiceTerminalState::Failed, Some("first".to_owned()))
            .await;
        hooks
            .service_terminal("b", ServiceTerminalState::Failed, Some("second".to_owned()))
            .await;

        let contents =
            std::fs::read_to_string(directory.path().join("failures.txt")).expect("marker");
        assert_eq!(contents.lines().collect::<Vec<_>>(), ["a:first"]);
    }

    #[test]
    fn hook_plan_rejects_unknown_selected_resource() {
        let declarations = vec![declaration(
            ComposeEventName::ServiceCompleted,
            Some(vec!["missing"]),
            None,
            "true",
        )];
        let error = build_hook_plans(&declarations, &["known".to_owned()], &[])
            .expect_err("unknown selector must fail");
        assert!(matches!(
            error,
            ComposeHookBuildError::UnknownService { name, .. } if name == "missing"
        ));
    }

    #[test]
    fn trigger_hook_without_declared_triggers_builds_but_never_fires() {
        let declarations = vec![declaration(
            ComposeEventName::TriggersSettled,
            None,
            None,
            "true",
        )];
        let plans =
            build_hook_plans(&declarations, &["worker".to_owned()], &[]).expect("build plans");
        assert!(plans[0].triggers.is_empty());
    }

    #[tokio::test]
    async fn trigger_restart_is_repeatable_but_aggregates_fire_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let declarations = vec![
            declaration(
                ComposeEventName::TriggerStarted,
                None,
                None,
                "echo started >> trigger-events.txt",
            ),
            declaration(
                ComposeEventName::TriggerRestarting,
                None,
                None,
                "echo restarting >> trigger-events.txt",
            ),
            declaration(
                ComposeEventName::TriggersStarted,
                None,
                None,
                "echo all-started >> trigger-events.txt",
            ),
            declaration(
                ComposeEventName::TriggersCompleted,
                None,
                None,
                "echo all-completed >> trigger-events.txt",
            ),
            declaration(
                ComposeEventName::TriggersSettled,
                None,
                None,
                "echo all-settled >> trigger-events.txt",
            ),
        ];
        let mut hooks = runtime(directory.path(), declarations, &["worker"], &["finite"]);

        hooks
            .trigger_transition("finite", TriggerLifecycleState::Running, 0, None)
            .await;
        hooks
            .trigger_transition("finite", TriggerLifecycleState::Restarting, 1, None)
            .await;
        hooks
            .trigger_transition("finite", TriggerLifecycleState::Running, 1, None)
            .await;
        hooks
            .trigger_transition("finite", TriggerLifecycleState::Completed, 1, None)
            .await;

        let contents =
            std::fs::read_to_string(directory.path().join("trigger-events.txt")).expect("marker");
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            [
                "started",
                "all-started",
                "restarting",
                "started",
                "all-completed",
                "all-settled"
            ]
        );
    }
}
