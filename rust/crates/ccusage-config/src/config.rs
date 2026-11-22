use std::{cell::RefCell, collections::BTreeMap, path::PathBuf};

use ccusage_cli::{ContextThresholds, NamedPiStore};

mod apply;
mod convert;
mod discovery;
mod pi_stores;
mod pricing;

use discovery::{detect_config_command, load_config_value, scan_config_path};

struct ConfigCommand {
    raw: String,
    agent: Option<String>,
    report: String,
}

pub struct ConfigContext {
    command: ConfigCommand,
    pi_stores: Vec<NamedPiStore>,
    pi_stores_error: Option<String>,
    error: Option<String>,
    layers: BTreeMap<String, crate::layers::Layer>,
    warnings: RefCell<Vec<String>>,
    config_origin: Option<(PathBuf, bool)>,
}

impl ConfigContext {
    pub fn from_args(args: &[String]) -> Self {
        let command = detect_config_command(args);
        if args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "-v" | "-V" | "--version"))
        {
            return Self::empty(command);
        }
        let explicit_path = scan_config_path(args);
        let (file, error, warnings) = load_config_value(explicit_path.as_deref());
        let mut context = Self::empty(command);
        context.error = error;
        context.warnings = RefCell::new(warnings);
        if explicit_path.is_none()
            && context.command.report == "statusline"
            && let Some(error) = context.error.take()
        {
            context.warnings.get_mut().push(error);
        }
        if let Some(file) = file {
            context.pi_stores = file.pi_stores;
            context.pi_stores_error = file.pi_stores_error;
            context.layers = file.layers;
            context.config_origin = Some((file.path, explicit_path.is_some()));
        }
        context
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings.borrow().clone()
    }

    fn empty(command: ConfigCommand) -> Self {
        Self {
            command,
            pi_stores: Vec::new(),
            pi_stores_error: None,
            error: None,
            layers: BTreeMap::new(),
            warnings: RefCell::new(Vec::new()),
            config_origin: None,
        }
    }

    pub fn for_dependency(&self, agent: &str) -> Self {
        let report = &self.command.report;
        Self {
            command: ConfigCommand {
                raw: format!("{agent} {report}"),
                agent: Some(agent.to_string()),
                report: report.clone(),
            },
            pi_stores: self.pi_stores.clone(),
            pi_stores_error: self.pi_stores_error.clone(),
            error: self.error.clone(),
            layers: self.layers.clone(),
            warnings: self.warnings.clone(),
            config_origin: self.config_origin.clone(),
        }
    }

    fn resolve_context_thresholds(
        &self,
        low_override: Option<u8>,
        medium_override: Option<u8>,
    ) -> Result<ContextThresholds, String> {
        let mut lows = Vec::new();
        let mut mediums = Vec::new();
        for (index, path) in self.option_paths().into_iter().enumerate() {
            if let Some(layer) = self.layers.get(&path) {
                if let Some(value) = layer.statusline.context_low_threshold {
                    lows.push((index, path.clone(), value));
                }
                if let Some(value) = layer.statusline.context_medium_threshold {
                    mediums.push((index, path, value));
                }
            }
        }
        let defaults = ContextThresholds::default();
        loop {
            let low = low_override
                .or_else(|| lows.last().map(|(_, _, value)| *value))
                .unwrap_or(defaults.low());
            let medium = medium_override
                .or_else(|| mediums.last().map(|(_, _, value)| *value))
                .unwrap_or(defaults.medium());
            match ContextThresholds::new(low, medium) {
                Ok(thresholds) => {
                    return Ok(thresholds);
                }
                Err(error) => {
                    let drop_low = match (low_override, medium_override) {
                        (Some(_), None) if !mediums.is_empty() => false,
                        (None, Some(_)) if !lows.is_empty() => true,
                        (None, None) => lows.last().is_some_and(|(index, _, _)| {
                            mediums.last().is_none_or(|(other, _, _)| index >= other)
                        }),
                        _ => return Err(error),
                    };
                    let (field, invalid) = if drop_low {
                        ("contextLowThreshold", lows.pop())
                    } else {
                        ("contextMediumThreshold", mediums.pop())
                    };
                    let Some((_, path, _)) = invalid else {
                        return Err(config_error(error));
                    };
                    let Some((file, strict)) = &self.config_origin else {
                        return Err(error);
                    };
                    let message = format!("{}: {path}.{field}: {error}", file.display());
                    if *strict {
                        return Err(config_error(message));
                    }
                    self.warnings
                        .borrow_mut()
                        .push(format!("Invalid ccusage config field: {message}"));
                }
            }
        }
    }

    fn warn_discovered_statusline_field(&self, path: &str, field: &str, message: &str) -> bool {
        let Some((file, false)) = &self.config_origin else {
            return false;
        };
        self.warnings.borrow_mut().push(format!(
            "Invalid ccusage config field: {}: {path}.{field}: {message}",
            file.display()
        ));
        true
    }

    fn option_paths(&self) -> Vec<String> {
        let report = &self.command.report;
        let mut paths = vec!["defaults".to_string()];
        if let Some(agent) = &self.command.agent {
            paths.extend([
                format!("commands.{report}"),
                format!("commands.{}", self.command.raw),
                format!("commands.{agent}:{report}"),
                format!("{agent}.defaults"),
                format!("{agent}.commands.{report}"),
            ]);
        } else {
            paths.push(format!("commands.{}", self.command.raw));
        }
        paths
    }

    fn timezone_setting(&self) -> Option<(String, &str)> {
        self.option_paths().into_iter().rev().find_map(|path| {
            let layer = self.layers.get(&path)?;
            let value = if self.command.report == "statusline" {
                &layer.statusline.timezone
            } else {
                &layer.shared.timezone
            };
            Some((path, value.as_deref()?))
        })
    }

    fn option_maps(&self) -> Vec<&crate::layers::Layer> {
        if self.layers.is_empty() {
            return Vec::new();
        }
        self.option_paths()
            .iter()
            .filter_map(|path| self.layers.get(path))
            .collect()
    }
}

fn config_error(message: impl Into<String>) -> String {
    format!("Invalid ccusage config: {}", message.into())
}

#[cfg(test)]
mod tests;
