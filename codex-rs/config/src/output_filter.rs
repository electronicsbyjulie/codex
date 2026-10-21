//! Process-wide accessibility filtering, selected only by the base user layer.
//!
//! Do not print patterns, source text, or regex parser diagnostics in errors.

use std::io;
use std::ops::Range;
use std::sync::OnceLock;

use regex_lite::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::ConfigLayerEntry;
use crate::ConfigLayerSource;
use crate::ConfigLayerStack;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputFilterConfig {
    #[serde(default)]
    pub rules: Vec<OutputFilterRule>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputFilterRule {
    /// Rust regex syntax. Matching is case-sensitive unless `(?i)` is present.
    pub pattern: String,
    /// An optional regex that must match the entire candidate to exempt it.
    pub exempt_pattern: Option<String>,
    /// Literal replacement text; capture expansion is intentionally disabled.
    #[serde(default = "default_replacement")]
    pub replacement: String,
}

impl std::fmt::Debug for OutputFilterRule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OutputFilterRule { .. }")
    }
}

fn default_replacement() -> String {
    "[filtered]".to_owned()
}

struct CompiledRule {
    pattern: Regex,
    exempt: Option<Regex>,
    replacement: String,
}

pub struct OutputFilter {
    config: OutputFilterConfig,
    rules: Vec<CompiledRule>,
}

static GLOBAL: OnceLock<OutputFilter> = OnceLock::new();

pub fn global() -> Option<&'static OutputFilter> {
    GLOBAL.get()
}

/// Projects, profiles, and session flags cannot remove this accessibility rule.
/// Changing an installed rule requires a process restart.
pub fn initialize_from_layers(layers: &ConfigLayerStack) -> io::Result<()> {
    let config = config_from_layers(layers.layers_low_to_high())?;
    install(config)
}

fn config_from_layers<'a>(
    mut layers: impl Iterator<Item = &'a ConfigLayerEntry>,
) -> io::Result<OutputFilterConfig> {
    layers
        .find(|layer| matches!(layer.name, ConfigLayerSource::User { profile: None, .. }))
        .and_then(|layer| layer.config.get("output_filter"))
        .cloned()
        .map(|value| value.try_into::<OutputFilterConfig>())
        .transpose()
        .map_err(|_| invalid_config())
        .map(Option::unwrap_or_default)
}

fn install(config: OutputFilterConfig) -> io::Result<()> {
    if let Some(current) = global() {
        return if current.config == config {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Restart Codex after changing the global output filter.",
            ))
        };
    }
    if !config.rules.is_empty() {
        let compiled = OutputFilter::compile(config)?;
        // Concurrent startup must agree on the same process-wide policy.
        if let Err(other) = GLOBAL.set(compiled)
            && global().is_some_and(|current| current.config != other.config)
        {
            return Err(invalid_config());
        }
    }
    Ok(())
}

fn invalid_config() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Invalid global output filter configuration.",
    )
}

impl OutputFilter {
    pub fn compile(config: OutputFilterConfig) -> io::Result<Self> {
        let rules = config
            .rules
            .iter()
            .map(|rule| {
                let pattern = Regex::new(&rule.pattern).map_err(|_| invalid_config())?;
                if pattern.is_match("") {
                    return Err(invalid_config());
                }
                let exempt = rule
                    .exempt_pattern
                    .as_ref()
                    .map(|pattern| {
                        Regex::new(&format!("\\A(?:{pattern})\\z")).map_err(|_| invalid_config())
                    })
                    .transpose()?;
                Ok(CompiledRule {
                    pattern,
                    exempt,
                    replacement: rule.replacement.clone(),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let filter = Self { config, rules };
        if filter
            .rules
            .iter()
            .any(|rule| filter.is_match(&rule.replacement))
        {
            return Err(invalid_config());
        }
        Ok(filter)
    }

    pub fn ranges(&self, text: &str) -> Vec<Range<usize>> {
        self.rules
            .iter()
            .flat_map(|rule| {
                rule.pattern.find_iter(text).filter_map(move |found| {
                    (!rule
                        .exempt
                        .as_ref()
                        .is_some_and(|exempt| exempt.is_match(found.as_str())))
                    .then_some(found.range())
                })
            })
            .collect()
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.rules.iter().any(|rule| {
            rule.pattern.find_iter(text).any(|found| {
                !rule
                    .exempt
                    .as_ref()
                    .is_some_and(|exempt| exempt.is_match(found.as_str()))
            })
        })
    }

    pub fn text(&self, text: &str) -> String {
        let mut text = text.to_owned();
        // Replacement can create a new match across its boundaries or for an
        // earlier rule. Recheck the whole result; never return a residual match.
        for _ in 0..8 {
            if !self.is_match(&text) {
                return text;
            }
            for rule in &self.rules {
                text = rule
                    .pattern
                    .replace_all(&text, |captures: &regex_lite::Captures<'_>| {
                        let found = &captures[0];
                        if rule
                            .exempt
                            .as_ref()
                            .is_some_and(|exempt| exempt.is_match(found))
                        {
                            found.to_owned()
                        } else {
                            rule.replacement.clone()
                        }
                    })
                    .into_owned();
            }
        }
        if self.is_match(&text) {
            String::new()
        } else {
            text
        }
    }

    /// Preserve wire keys, discriminators, and correlation identifiers. If one
    /// itself matches, reject the envelope instead of corrupting the protocol.
    pub fn json(&self, value: &mut Value) -> Result<(), FilteredStructure> {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(values) => {
                for value in values {
                    self.json(value)?;
                }
            }
            Value::Object(values) => {
                for (key, value) in values {
                    if self.is_match(key) {
                        return Err(FilteredStructure);
                    }
                    if structural_key(key) {
                        if self.contains_json(value) {
                            return Err(FilteredStructure);
                        }
                    } else {
                        self.json(value)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn contains_json(&self, value: &Value) -> bool {
        match value {
            Value::String(text) => self.is_match(text),
            Value::Array(values) => values.iter().any(|value| self.contains_json(value)),
            Value::Object(values) => values
                .iter()
                .any(|(key, value)| self.is_match(key) || self.contains_json(value)),
            _ => false,
        }
    }

    /// A filtered typed value must still satisfy its original wire schema.
    pub fn typed<T: Serialize + DeserializeOwned>(&self, value: &T) -> Option<T> {
        let mut json = serde_json::to_value(value).ok()?;
        self.json(&mut json).ok()?;
        serde_json::from_value(json).ok()
    }
}

#[derive(Debug)]
pub struct FilteredStructure;

fn structural_key(key: &str) -> bool {
    matches!(
        key,
        "type" | "method" | "id" | "role" | "channel" | "status" | "kind"
    ) || key.ends_with("_id")
        || key.ends_with("Id")
}

/// Apply the same policy to every app-server transport, including history RPCs.
pub fn wire_json(json: String) -> Option<String> {
    let Some(filter) = global() else {
        return Some(json);
    };
    let mut value: Value = serde_json::from_str(&json).ok()?;
    // Arbitrary regexes have no finite safe chunk size. Complete item events
    // carry the text; do not leak a match assembled from several delta events.
    if value
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| method.ends_with("/delta") || method.ends_with("_delta"))
    {
        return None;
    }
    filter.json(&mut value).ok()?;
    serde_json::to_string(&value).ok()
}

#[cfg(test)]
#[path = "output_filter_tests.rs"]
mod tests;
