//! Skills: named, typed, async abilities an agent can execute on demand.
//!
//! Implement [`Skill`] for full control, or wrap an async closure with
//! [`FnSkill`] for quick one-offs. Skills are registered on the agent via
//! [`AgentBuilder::skill`](crate::agent::AgentBuilder::skill) and invoked by
//! name — locally, over REST (`POST /skills/{name}`), WebSocket, or gRPC.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};

/// A named async ability with a JSON-in / JSON-out contract.
#[async_trait::async_trait]
pub trait Skill: Send + Sync {
    /// Unique skill name (used for lookup and routing).
    fn name(&self) -> &str;

    /// Human-readable description of what the skill does.
    fn description(&self) -> &str;

    /// JSON Schema for the expected input. Defaults to an unconstrained object.
    fn input_schema(&self) -> Value {
        serde_json::json!({ "type": "object" })
    }

    /// Permissions this skill needs (free-form labels such as `"net"`,
    /// `"fs:read"`). The agent's [`SkillPolicy`] must grant all of them or
    /// execution is refused. Defaults to none.
    fn required_permissions(&self) -> Vec<String> {
        Vec::new()
    }

    /// Execute the skill.
    async fn execute(&self, input: Value) -> Result<Value>;
}

/// The agent-level sandbox for skill execution: which skills may run, which
/// permissions are granted, and how long a skill may take.
///
/// The default policy allows every registered skill, grants no permissions
/// (so skills that declare [`Skill::required_permissions`] are refused until
/// granted), and applies a 30-second timeout.
///
/// ```
/// use corrosive_agents::skills::SkillPolicy;
/// use std::time::Duration;
///
/// let policy = SkillPolicy::new()
///     .allow_only(["fetch", "summarize"]) // everything else is refused
///     .grant("net")                       // satisfy `required_permissions`
///     .with_timeout(Duration::from_secs(5));
/// ```
#[derive(Debug, Clone)]
pub struct SkillPolicy {
    allowed_skills: Option<std::collections::HashSet<String>>,
    granted_permissions: std::collections::HashSet<String>,
    timeout: Option<std::time::Duration>,
}

impl Default for SkillPolicy {
    fn default() -> Self {
        Self {
            allowed_skills: None,
            granted_permissions: std::collections::HashSet::new(),
            timeout: Some(std::time::Duration::from_secs(30)),
        }
    }
}

impl SkillPolicy {
    /// The default policy (all skills allowed, no permissions granted,
    /// 30-second timeout).
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict execution to an explicit allowlist of skill names.
    #[must_use]
    pub fn allow_only<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_skills = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// Grant a permission label (see [`Skill::required_permissions`]).
    #[must_use]
    pub fn grant(mut self, permission: impl Into<String>) -> Self {
        self.granted_permissions.insert(permission.into());
        self
    }

    /// Cap how long a single skill execution may run.
    #[must_use]
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Remove the execution timeout.
    #[must_use]
    pub fn without_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }

    /// The configured timeout, if any.
    pub fn timeout(&self) -> Option<std::time::Duration> {
        self.timeout
    }

    /// Check whether `skill` may run under this policy.
    pub fn check(&self, skill: &dyn Skill) -> Result<()> {
        if let Some(allowed) = &self.allowed_skills {
            if !allowed.contains(skill.name()) {
                return Err(Error::PermissionDenied(format!(
                    "skill '{}' is not on the allowlist",
                    skill.name()
                )));
            }
        }
        for permission in skill.required_permissions() {
            if !self.granted_permissions.contains(&permission) {
                return Err(Error::PermissionDenied(format!(
                    "skill '{}' requires permission '{permission}' which is not granted",
                    skill.name()
                )));
            }
        }
        Ok(())
    }
}

type SkillFuture = Pin<Box<dyn Future<Output = Result<Value>> + Send>>;
type SkillFn = dyn Fn(Value) -> SkillFuture + Send + Sync;

/// A [`Skill`] built from an async closure.
///
/// ```
/// use corrosive_agents::skills::FnSkill;
/// use serde_json::json;
///
/// let shout = FnSkill::new("shout", "Uppercases the input text", |input| async move {
///     let text = input["text"].as_str().unwrap_or_default();
///     Ok(json!({ "text": text.to_uppercase() }))
/// });
/// ```
pub struct FnSkill {
    name: String,
    description: String,
    schema: Value,
    permissions: Vec<String>,
    handler: Arc<SkillFn>,
}

impl FnSkill {
    /// Create a skill from a name, description, and async closure.
    pub fn new<F, Fut>(name: impl Into<String>, description: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            schema: serde_json::json!({ "type": "object" }),
            permissions: Vec::new(),
            handler: Arc::new(move |input| Box::pin(handler(input))),
        }
    }

    /// Attach a JSON Schema describing the expected input.
    #[must_use]
    pub fn with_schema(mut self, schema: Value) -> Self {
        self.schema = schema;
        self
    }

    /// Declare permissions this skill requires (must be granted by the
    /// agent's [`SkillPolicy`] before the skill may run).
    #[must_use]
    pub fn with_permissions<I, S>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }
}

#[async_trait::async_trait]
impl Skill for FnSkill {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.schema.clone()
    }

    fn required_permissions(&self) -> Vec<String> {
        self.permissions.clone()
    }

    async fn execute(&self, input: Value) -> Result<Value> {
        (self.handler)(input).await
    }
}

/// A registry mapping skill names to implementations.
#[derive(Default, Clone)]
pub struct SkillRegistry {
    skills: HashMap<String, Arc<dyn Skill>>,
}

impl SkillRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a skill, replacing any previous skill with the same name.
    pub fn register(&mut self, skill: Arc<dyn Skill>) {
        self.skills.insert(skill.name().to_string(), skill);
    }

    /// Look up a skill by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Skill>> {
        self.skills.get(name).cloned()
    }

    /// Execute a registered skill by name.
    pub async fn execute(&self, name: &str, input: Value) -> Result<Value> {
        let skill = self
            .get(name)
            .ok_or_else(|| Error::SkillNotFound(name.to_string()))?;
        skill.execute(input).await
    }

    /// All registered skills, in arbitrary order.
    pub fn list(&self) -> Vec<Arc<dyn Skill>> {
        self.skills.values().cloned().collect()
    }

    /// Number of registered skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// `true` when no skills are registered.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

impl std::fmt::Debug for SkillRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillRegistry")
            .field("skills", &self.skills.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn fn_skill_executes() {
        let mut registry = SkillRegistry::new();
        registry.register(Arc::new(FnSkill::new(
            "double",
            "Doubles n",
            |input| async move {
                let n = input["n"].as_i64().unwrap_or(0);
                Ok(json!({ "n": n * 2 }))
            },
        )));

        let out = registry
            .execute("double", json!({ "n": 21 }))
            .await
            .unwrap();
        assert_eq!(out["n"], 42);
    }

    #[tokio::test]
    async fn missing_skill_errors() {
        let registry = SkillRegistry::new();
        let err = registry.execute("nope", json!({})).await.unwrap_err();
        assert!(matches!(err, Error::SkillNotFound(_)));
    }
}
