/// A top-level `arg <name> [= "<default>"]` declaration.
///
/// Args are string-valued Iterfile parameters resolved at load time.
/// They are exposed to templates as `{{arg.<name>}}` and can be
/// overridden via `iter run --arg name=value` or compose service
/// `args { name = "value" }` blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgDecl {
    /// Argument name. Must be a valid identifier.
    pub name: String,
    /// Default value. `None` means the arg is required and must be
    /// supplied via CLI or compose override.
    pub default: Option<String>,
}
