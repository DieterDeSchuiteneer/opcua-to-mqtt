use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::de::{self, Deserializer};
use serde::Deserialize;

/// A config value that may be a literal, an environment variable reference,
/// or a reference into AWS Secrets Manager. Parsed from a plain string using
/// a prefix convention so it works with vanilla `serde_yaml` (no custom YAML
/// tags):
///
/// - `env:NAME`                 -> read env var NAME
/// - `aws-secret:<id>#<field>`  -> fetch secret <id>, extract JSON field <field>
/// - anything else              -> used as-is
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRef {
    Plain(String),
    Env(String),
    AwsSecret { secret_id: String, field: String },
}

impl SecretRef {
    pub fn parse(raw: &str) -> Self {
        if let Some(name) = raw.strip_prefix("env:") {
            return SecretRef::Env(name.to_string());
        }
        if let Some(rest) = raw.strip_prefix("aws-secret:") {
            if let Some((secret_id, field)) = rest.split_once('#') {
                return SecretRef::AwsSecret {
                    secret_id: secret_id.to_string(),
                    field: field.to_string(),
                };
            }
        }
        SecretRef::Plain(raw.to_string())
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(SecretRef::parse(&raw))
    }
}

#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve(&self, secret: &SecretRef) -> Result<String>;
}

/// Resolves `Plain` and `Env` refs directly. `AwsSecret` refs are delegated
/// to an optional AWS backend, which is only present when the crate is built
/// with the `aws-secrets` feature and `secrets.aws.enabled: true`.
pub struct DefaultResolver {
    #[cfg(feature = "aws-secrets")]
    aws: Option<AwsSecretsBackend>,
    #[cfg(not(feature = "aws-secrets"))]
    _private: (),
}

impl DefaultResolver {
    #[cfg(feature = "aws-secrets")]
    pub fn new(aws: Option<AwsSecretsBackend>) -> Self {
        Self { aws }
    }

    #[cfg(not(feature = "aws-secrets"))]
    pub fn new() -> Self {
        Self { _private: () }
    }
}

#[async_trait]
impl SecretResolver for DefaultResolver {
    async fn resolve(&self, secret: &SecretRef) -> Result<String> {
        match secret {
            SecretRef::Plain(v) => Ok(v.clone()),
            SecretRef::Env(name) => std::env::var(name)
                .with_context(|| format!("environment variable {name} is not set")),
            SecretRef::AwsSecret { secret_id, field } => {
                #[cfg(feature = "aws-secrets")]
                {
                    let backend = self.aws.as_ref().ok_or_else(|| {
                        anyhow!(
                            "secret 'aws-secret:{secret_id}#{field}' requires secrets.aws.enabled: true"
                        )
                    })?;
                    backend.fetch_field(secret_id, field).await
                }
                #[cfg(not(feature = "aws-secrets"))]
                {
                    Err(anyhow!(
                        "secret 'aws-secret:{secret_id}#{field}' requires building with --features aws-secrets"
                    ))
                }
            }
        }
    }
}

#[cfg(feature = "aws-secrets")]
pub struct AwsSecretsBackend {
    client: aws_sdk_secretsmanager::Client,
    cache: Mutex<HashMap<String, serde_json::Value>>,
}

#[cfg(feature = "aws-secrets")]
impl AwsSecretsBackend {
    pub async fn new(region: Option<String>) -> Self {
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
        if let Some(region) = region {
            loader = loader.region(aws_config::Region::new(region));
        }
        let sdk_config = loader.load().await;
        Self {
            client: aws_sdk_secretsmanager::Client::new(&sdk_config),
            cache: Mutex::new(HashMap::new()),
        }
    }

    async fn fetch_field(&self, secret_id: &str, field: &str) -> Result<String> {
        let value = self.fetch_secret(secret_id).await?;
        value
            .get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("secret '{secret_id}' has no string field '{field}'"))
    }

    async fn fetch_secret(&self, secret_id: &str) -> Result<serde_json::Value> {
        if let Some(cached) = self.cache.lock().unwrap().get(secret_id) {
            return Ok(cached.clone());
        }

        let response = self
            .client
            .get_secret_value()
            .secret_id(secret_id)
            .send()
            .await
            .with_context(|| format!("fetching secret '{secret_id}' from AWS Secrets Manager"))?;
        let raw = response
            .secret_string()
            .ok_or_else(|| anyhow!("secret '{secret_id}' has no string value"))?;
        let parsed: serde_json::Value = serde_json::from_str(raw)
            .with_context(|| format!("secret '{secret_id}' is not a JSON object"))?;

        self.cache
            .lock()
            .unwrap()
            .insert(secret_id.to_string(), parsed.clone());
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_ref() {
        assert_eq!(
            SecretRef::parse("env:MQTT_PASSWORD"),
            SecretRef::Env("MQTT_PASSWORD".to_string())
        );
    }

    #[test]
    fn parses_aws_secret_ref() {
        assert_eq!(
            SecretRef::parse("aws-secret:prod/mqtt#password"),
            SecretRef::AwsSecret {
                secret_id: "prod/mqtt".to_string(),
                field: "password".to_string(),
            }
        );
    }

    #[test]
    fn falls_back_to_plain() {
        assert_eq!(
            SecretRef::parse("hunter2"),
            SecretRef::Plain("hunter2".to_string())
        );
    }

    #[test]
    fn aws_secret_without_field_is_plain() {
        // missing '#field' doesn't match the aws-secret shape, so it's
        // treated as a literal rather than silently truncated.
        assert_eq!(
            SecretRef::parse("aws-secret:prod/mqtt"),
            SecretRef::Plain("aws-secret:prod/mqtt".to_string())
        );
    }

    #[tokio::test]
    async fn env_resolver_reads_var() {
        std::env::set_var("SECRET_TEST_VAR", "value123");
        #[cfg(feature = "aws-secrets")]
        let resolver = DefaultResolver::new(None);
        #[cfg(not(feature = "aws-secrets"))]
        let resolver = DefaultResolver::new();
        let resolved = resolver
            .resolve(&SecretRef::Env("SECRET_TEST_VAR".to_string()))
            .await
            .unwrap();
        assert_eq!(resolved, "value123");
    }
}
