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

    /// Execute the skill.
    async fn execute(&self, input: Value) -> Result<Value>;
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
            handler: Arc::new(move |input| Box::pin(handler(input))),
        }
    }

    /// Attach a JSON Schema describing the expected input.
    #[must_use]
    pub fn with_schema(mut self, schema: Value) -> Self {
        self.schema = schema;
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
