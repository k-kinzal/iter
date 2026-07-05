use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{
    push_each, push_enum, push_flag, push_opt, push_pair, push_paths, push_positional_boundary,
};
use crate::values::Switch;

/// Plugin installation scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginScope {
    /// `user`.
    User,
    /// `project`.
    Project,
    /// `local`.
    Local,
    /// `managed`.
    Managed,
}

impl PluginScope {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Managed => "managed",
        }
    }
}

/// Components supported by `claude plugin init --with`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginComponent {
    /// `skills`.
    Skills,
    /// `agents`.
    Agents,
    /// `hooks`.
    Hooks,
    /// `mcp`.
    Mcp,
    /// `lsp`.
    Lsp,
    /// `output-style`.
    OutputStyle,
    /// `channel`.
    Channel,
}

impl PluginComponent {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Agents => "agents",
            Self::Hooks => "hooks",
            Self::Mcp => "mcp",
            Self::Lsp => "lsp",
            Self::OutputStyle => "output-style",
            Self::Channel => "channel",
        }
    }
}

/// `claude plugin ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Plugin {
    /// `plugin details`.
    Details { name: String },
    /// `plugin disable`.
    Disable(PluginDisable),
    /// `plugin enable`.
    Enable(PluginEnable),
    /// `plugin help [command]`.
    Help(Option<PluginHelpCommand>),
    /// `plugin init`.
    Init(PluginInit),
    /// `plugin install`.
    Install(PluginInstall),
    /// `plugin list`.
    List(PluginList),
    /// `plugin marketplace ...`.
    Marketplace(PluginMarketplace),
    /// `plugin prune`.
    Prune(PluginPrune),
    /// `plugin tag`.
    Tag(PluginTag),
    /// `plugin uninstall`.
    Uninstall(PluginUninstall),
    /// `plugin update`.
    Update(PluginUpdate),
    /// `plugin validate`.
    Validate(PluginValidate),
}

impl Plugin {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Details { name } => {
                args.push("details".into());
                args.push(name.into());
            }
            Self::Disable(command) => {
                args.push("disable".into());
                command.render(args);
            }
            Self::Enable(command) => {
                args.push("enable".into());
                command.render(args);
            }
            Self::Help(command) => {
                args.push("help".into());
                if let Some(command) = command {
                    command.render(args);
                }
            }
            Self::Init(command) => {
                args.push("init".into());
                command.render(args);
            }
            Self::Install(command) => {
                args.push("install".into());
                command.render(args);
            }
            Self::List(command) => {
                args.push("list".into());
                command.render(args);
            }
            Self::Marketplace(command) => {
                args.push("marketplace".into());
                command.render(args);
            }
            Self::Prune(command) => {
                args.push("prune".into());
                command.render(args);
            }
            Self::Tag(command) => {
                args.push("tag".into());
                command.render(args);
            }
            Self::Uninstall(command) => {
                args.push("uninstall".into());
                command.render(args);
            }
            Self::Update(command) => {
                args.push("update".into());
                command.render(args);
            }
            Self::Validate(command) => {
                args.push("validate".into());
                command.render(args);
            }
        }
    }
}

/// `claude plugin help`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginHelpCommand {
    /// A direct plugin subcommand.
    Direct(PluginDirectHelpCommand),
    /// A marketplace subcommand.
    Marketplace(Option<PluginMarketplaceHelpCommand>),
}

impl PluginHelpCommand {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Direct(command) => args.push(command.as_str().into()),
            Self::Marketplace(command) => {
                args.push("marketplace".into());
                if let Some(command) = command {
                    args.push(command.as_str().into());
                }
            }
        }
    }
}

/// `claude plugin help <command>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginDirectHelpCommand {
    /// `details`.
    Details,
    /// `disable`.
    Disable,
    /// `enable`.
    Enable,
    /// `init`.
    Init,
    /// `install`.
    Install,
    /// `list`.
    List,
    /// `prune`.
    Prune,
    /// `tag`.
    Tag,
    /// `uninstall`.
    Uninstall,
    /// `update`.
    Update,
    /// `validate`.
    Validate,
}

impl PluginDirectHelpCommand {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Details => "details",
            Self::Disable => "disable",
            Self::Enable => "enable",
            Self::Init => "init",
            Self::Install => "install",
            Self::List => "list",
            Self::Prune => "prune",
            Self::Tag => "tag",
            Self::Uninstall => "uninstall",
            Self::Update => "update",
            Self::Validate => "validate",
        }
    }
}

/// `claude plugin disable`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginDisable {
    /// Plugin target.
    pub target: Option<PluginDisableTarget>,
    /// `--scope`.
    pub scope: Option<PluginScope>,
}

impl PluginDisable {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_enum(args, "--scope", self.scope.map(PluginScope::as_str));
        match &self.target {
            Some(PluginDisableTarget::All) => args.push("--all".into()),
            Some(PluginDisableTarget::Plugin(plugin)) => args.push(plugin.into()),
            None => {}
        }
    }
}

/// Mutually exclusive `claude plugin disable` targets.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginDisableTarget {
    /// Disable one plugin.
    Plugin(String),
    /// Disable all enabled plugins.
    All,
}

/// `claude plugin enable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginEnable {
    /// Plugin name.
    pub plugin: String,
    /// `--scope`.
    pub scope: Option<PluginScope>,
}

impl PluginEnable {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_enum(args, "--scope", self.scope.map(PluginScope::as_str));
        args.push((&self.plugin).into());
    }
}

/// `claude plugin init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInit {
    /// Plugin name.
    pub name: String,
    /// `--author`.
    pub author: Option<String>,
    /// `--author-email`.
    pub author_email: Option<String>,
    /// `--description`.
    pub description: Option<String>,
    /// `--force`.
    pub force: Switch,
    /// `--with`.
    pub with: Vec<PluginComponent>,
}

impl PluginInit {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_opt(args, "--author", self.author.as_deref());
        push_opt(args, "--author-email", self.author_email.as_deref());
        push_opt(args, "--description", self.description.as_deref());
        push_flag(args, self.force, "--force");
        for component in &self.with {
            push_pair(args, "--with", component.as_str());
        }
        args.push((&self.name).into());
    }
}

/// `claude plugin install`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstall {
    /// Plugin selector.
    pub plugin: String,
    /// `--config`.
    pub config: Vec<String>,
    /// `--scope`.
    pub scope: Option<PluginScope>,
}

impl PluginInstall {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_each(args, "--config", &self.config);
        push_enum(args, "--scope", self.scope.map(PluginScope::as_str));
        args.push((&self.plugin).into());
    }
}

/// `claude plugin list`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginList {
    /// `--available`.
    pub available: Switch,
    /// `--json`.
    pub json: Switch,
}

impl PluginList {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.available, "--available");
        push_flag(args, self.json, "--json");
    }
}

/// `claude plugin marketplace ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginMarketplace {
    /// `marketplace add`.
    Add(PluginMarketplaceAdd),
    /// `marketplace help [command]`.
    Help(Option<PluginMarketplaceHelpCommand>),
    /// `marketplace list`.
    List(PluginMarketplaceList),
    /// `marketplace remove`.
    Remove(PluginMarketplaceRemove),
    /// `marketplace update`.
    Update(PluginMarketplaceUpdate),
}

impl PluginMarketplace {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Add(command) => {
                args.push("add".into());
                command.render(args);
            }
            Self::Help(command) => {
                args.push("help".into());
                if let Some(command) = command {
                    args.push(command.as_str().into());
                }
            }
            Self::List(command) => {
                args.push("list".into());
                command.render(args);
            }
            Self::Remove(command) => {
                args.push("remove".into());
                command.render(args);
            }
            Self::Update(command) => {
                args.push("update".into());
                command.render(args);
            }
        }
    }
}

/// `claude plugin marketplace help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginMarketplaceHelpCommand {
    /// `add`.
    Add,
    /// `list`.
    List,
    /// `remove`.
    Remove,
    /// `update`.
    Update,
}

impl PluginMarketplaceHelpCommand {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::List => "list",
            Self::Remove => "remove",
            Self::Update => "update",
        }
    }
}

/// `claude plugin marketplace add`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMarketplaceAdd {
    /// Source URL/path/repo.
    pub source: String,
    /// `--scope`.
    pub scope: Option<PluginScope>,
    /// `--sparse`.
    pub sparse: Vec<PathBuf>,
}

impl PluginMarketplaceAdd {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_enum(args, "--scope", self.scope.map(PluginScope::as_str));
        push_paths(args, "--sparse", &self.sparse);
        push_positional_boundary(args);
        args.push((&self.source).into());
    }
}

/// `claude plugin marketplace list`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginMarketplaceList {
    /// `--json`.
    pub json: Switch,
}

impl PluginMarketplaceList {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.json, "--json");
    }
}

/// `claude plugin marketplace remove`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMarketplaceRemove {
    /// Marketplace name.
    pub name: String,
    /// `--scope`.
    pub scope: Option<PluginScope>,
}

impl PluginMarketplaceRemove {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_enum(args, "--scope", self.scope.map(PluginScope::as_str));
        args.push((&self.name).into());
    }
}

/// `claude plugin marketplace update`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginMarketplaceUpdate {
    /// Optional marketplace name.
    pub name: Option<String>,
}

impl PluginMarketplaceUpdate {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        if let Some(name) = &self.name {
            args.push(name.into());
        }
    }
}

/// `claude plugin prune`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginPrune {
    /// `--dry-run`.
    pub dry_run: Switch,
    /// `--scope`.
    pub scope: Option<PluginScope>,
    /// `--yes`.
    pub yes: Switch,
}

impl PluginPrune {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.dry_run, "--dry-run");
        push_enum(args, "--scope", self.scope.map(PluginScope::as_str));
        push_flag(args, self.yes, "--yes");
    }
}

/// `claude plugin tag`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginTag {
    /// Optional plugin path.
    pub path: Option<PathBuf>,
    /// `--dry-run`.
    pub dry_run: Switch,
    /// `--force`.
    pub force: Switch,
    /// `--message`.
    pub message: Option<String>,
    /// `--push`.
    pub push: Switch,
    /// `--remote`.
    pub remote: Option<String>,
}

impl PluginTag {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.dry_run, "--dry-run");
        push_flag(args, self.force, "--force");
        push_opt(args, "--message", self.message.as_deref());
        push_flag(args, self.push, "--push");
        push_opt(args, "--remote", self.remote.as_deref());
        if let Some(path) = &self.path {
            args.push(path.into());
        }
    }
}

/// `claude plugin uninstall`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginUninstall {
    /// Plugin name.
    pub plugin: String,
    /// `--keep-data`.
    pub keep_data: Switch,
    /// `--prune`.
    pub prune: Switch,
    /// `--scope`.
    pub scope: Option<PluginScope>,
    /// `--yes`.
    pub yes: Switch,
}

impl PluginUninstall {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.keep_data, "--keep-data");
        push_flag(args, self.prune, "--prune");
        push_enum(args, "--scope", self.scope.map(PluginScope::as_str));
        push_flag(args, self.yes, "--yes");
        args.push((&self.plugin).into());
    }
}

/// `claude plugin update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginUpdate {
    /// Plugin name.
    pub plugin: String,
    /// `--scope`.
    pub scope: Option<PluginScope>,
}

impl PluginUpdate {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_enum(args, "--scope", self.scope.map(PluginScope::as_str));
        args.push((&self.plugin).into());
    }
}

/// `claude plugin validate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginValidate {
    /// Plugin or marketplace manifest path.
    pub path: PathBuf,
    /// `--strict`.
    pub strict: Switch,
}

impl PluginValidate {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.strict, "--strict");
        args.push((&self.path).into());
    }
}
