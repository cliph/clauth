//! Named endpoint + model templates a Setup-tab account can be stamped from.
//!
//! A preset carries exactly the two things that make an account talk to a given
//! provider: its `base_url` and its [`ModelSettings`]. Credentials, env, and
//! every fallback knob stay out — those are per-account, and a template that
//! carried them would silently move an api key between accounts.
//!
//! The built-ins ship in the binary; the rest live one JSON file per preset
//! under `~/.clauth/presets/`. The file NAME is the preset name, so it goes
//! through [`crate::actions::validate_profile_name`] (the same charset that
//! bounds a profile directory) before it ever reaches a path.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::profile::{ModelSettings, atomic_write_600, clauth_dir, read_json_file};

/// A named `base_url` + [`ModelSettings`] template.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Preset {
    pub(crate) name: String,
    pub(crate) base_url: Option<String>,
    pub(crate) models: ModelSettings,
    /// Ships in the binary: never written, never deleted, never overwritten.
    pub(crate) builtin: bool,
}

/// On-disk shape of `~/.clauth/presets/<name>.json`. The name is the file stem,
/// so it is deliberately absent from the body — one spelling, no way for the two
/// to disagree.
#[derive(Debug, Serialize, Deserialize)]
struct PresetFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(default)]
    models: ModelSettings,
}

/// One entry of the shipped table.
struct Builtin {
    name: &'static str,
    base_url: &'static str,
    binds: Binds,
}

/// How a built-in fills the account's model block.
enum Binds {
    /// One id per Claude tier. `default` becomes the `opus` alias, so the main
    /// model follows the opus bind and a model release touches tier rows only;
    /// `subagent` stays unset, so a subagent's own tier pick resolves through
    /// that tier's bind. A `[1m]` suffix makes Claude Code size the context
    /// window at 1M, so it rides only an id whose endpoint takes that much.
    Tiers {
        opus: &'static str,
        sonnet: &'static str,
        haiku: &'static str,
        fable: &'static str,
    },
    /// One id on `default`, every tier and the subagent row. Alibaba's
    /// `POST /apps/anthropic/v1/messages` answers `400 InvalidParameter "Model
    /// not exist."` to `claude-sonnet-4-5` and `claude-haiku-4-5-20251001`
    /// (measured 2026-08-11 against `token-plan.ap-southeast-1`), so a row left
    /// unset is a hard failure there; Alibaba's own `bl config agent --agent
    /// claude-code` writes every row for the same reason.
    Every(&'static str),
}

impl Binds {
    fn models(&self) -> ModelSettings {
        let id = |s: &str| Some(s.to_string());
        match *self {
            Binds::Tiers {
                opus,
                sonnet,
                haiku,
                fable,
            } => ModelSettings {
                default: id("opus"),
                opus: id(opus),
                sonnet: id(sonnet),
                haiku: id(haiku),
                fable: id(fable),
                subagent: None,
            },
            Binds::Every(model) => ModelSettings {
                default: id(model),
                opus: id(model),
                sonnet: id(model),
                haiku: id(model),
                fable: id(model),
                subagent: id(model),
            },
        }
    }
}

/// The built-ins, in menu order.
const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/anthropic",
        binds: Binds::Tiers {
            opus: "deepseek-v4-pro[1m]",
            sonnet: "deepseek-flash[1m]",
            haiku: "deepseek-flash[1m]",
            fable: "deepseek-v4-pro[1m]",
        },
    },
    Builtin {
        name: "Z.ai",
        base_url: "https://api.z.ai/api/anthropic",
        binds: Binds::Tiers {
            opus: "glm-5.3[1m]",
            sonnet: "glm-5.3-flash[1m]",
            haiku: "glm-5.3-flash[1m]",
            fable: "glm-5.3[1m]",
        },
    },
    Builtin {
        name: "OpenRouter",
        // OpenRouter's own Claude Code guide pins this exact base URL, so CC's
        // `/v1/messages` lands on `/api/v1/messages` (openrouter.ai docs,
        // "Connect Claude to OpenRouter").
        base_url: "https://openrouter.ai/api",
        // OpenRouter serves the real Claude models, so each tier binds its own
        // family's moving `-latest` alias instead of a pro/flash pair; the same
        // guide marks only the fable, opus and sonnet aliases `[1m]`-capable.
        binds: Binds::Tiers {
            opus: "~anthropic/claude-opus-latest[1m]",
            sonnet: "~anthropic/claude-sonnet-latest[1m]",
            haiku: "~anthropic/claude-haiku-latest",
            fable: "~anthropic/claude-fable-latest[1m]",
        },
    },
    Builtin {
        name: "MiniMax",
        // MiniMax fronts the Anthropic-shaped surface at `/anthropic`, so CC's
        // `/v1/messages` lands on `/anthropic/v1/messages` (platform.minimax.io,
        // "Claude Code" integration guide). The China-region host is a separate
        // account and endpoint; only the international one ships, matching what
        // `providers::minimax` claims.
        base_url: "https://api.minimax.io/anthropic",
        // The fast model is sold on the M Plan alone: a pay-as-you-go key
        // fails on sonnet and haiku until those two rows are re-pointed.
        binds: Binds::Tiers {
            opus: "MiniMax-M3[1m]",
            sonnet: "MiniMax-M3.1-Flash-Preview[1m]",
            haiku: "MiniMax-M3.1-Flash-Preview[1m]",
            fable: "MiniMax-M3[1m]",
        },
    },
    Builtin {
        name: "Qwen-TokenPlan-Intl",
        base_url: "https://token-plan.ap-southeast-1.maas.aliyuncs.com/apps/anthropic",
        binds: Binds::Every("qwen3.8-max"),
    },
    Builtin {
        name: "Qwen-TokenPlan-CN",
        base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic",
        binds: Binds::Every("qwen3.8-max"),
    },
    Builtin {
        name: "Qwen-CodingPlan-Intl",
        base_url: "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic",
        binds: Binds::Every("qwen3-coder-plus"),
    },
    Builtin {
        name: "Qwen-CodingPlan-CN",
        base_url: "https://coding.dashscope.aliyuncs.com/apps/anthropic",
        binds: Binds::Every("qwen3-coder-plus"),
    },
];

fn builtins() -> Vec<Preset> {
    BUILTINS
        .iter()
        .map(|b| Preset {
            name: b.name.to_string(),
            base_url: Some(b.base_url.to_string()),
            models: b.binds.models(),
            builtin: true,
        })
        .collect()
}

/// Whether `name` collides with a built-in. Case-insensitive: a
/// case-folding filesystem would let `deepseek.json` shadow the built-in's slot
/// on one host and not another, so the refusal can't depend on spelling.
pub(crate) fn is_builtin(name: &str) -> bool {
    BUILTINS
        .iter()
        .any(|b| b.name.eq_ignore_ascii_case(name.trim()))
}

fn presets_dir() -> Result<std::path::PathBuf> {
    Ok(clauth_dir()?.join("presets"))
}

/// The name's own path, refusing anything that isn't a bare filename. Reuses the
/// profile-name charset (`[A-Za-z0-9-_.@+]`, no leading `.`), which is what keeps
/// a separator or a `..` out of the join.
fn preset_path(name: &str) -> Result<std::path::PathBuf> {
    let trimmed = crate::actions::validate_name_chars(name)?;
    Ok(presets_dir()?.join(format!("{trimmed}.json")))
}

/// Built-ins first, then the on-disk ones sorted by name. A file that won't
/// parse is skipped rather than failing the whole list — one hand-edited preset
/// must not hide the others from the picker.
pub(crate) fn list_presets() -> Vec<Preset> {
    let mut out = builtins();
    let Ok(dir) = presets_dir() else {
        return out;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut custom: Vec<Preset> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // A built-in's slot is never readable from disk: the binary's copy is
        // the only definition, so a stray file with that stem stays invisible
        // instead of quietly shadowing it.
        if is_builtin(name) {
            continue;
        }
        let Ok(file) = read_json_file::<PresetFile>(&path) else {
            continue;
        };
        custom.push(Preset {
            name: name.to_string(),
            base_url: file.base_url,
            models: file.models,
            builtin: false,
        });
    }
    custom.sort_by_key(|a| a.name.to_lowercase());
    out.extend(custom);
    out
}

/// Built-ins first, then disk. `None` when neither carries the name.
pub(crate) fn load_preset(name: &str) -> Option<Preset> {
    let trimmed = name.trim();
    if let Some(p) = builtins()
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(trimmed))
    {
        return Some(p);
    }
    let path = preset_path(trimmed).ok()?;
    let file = read_json_file::<PresetFile>(&path).ok()?;
    Some(Preset {
        name: trimmed.to_string(),
        base_url: file.base_url,
        models: file.models,
        builtin: false,
    })
}

/// Whether a custom preset already occupies `name`. Callers confirm before
/// [`save_preset`] overwrites it.
pub(crate) fn preset_exists(name: &str) -> bool {
    preset_path(name).is_ok_and(|p| p.exists())
}

/// Write `name` to disk, replacing any custom preset already there. Refuses a
/// built-in name outright — the binary's copy is the definition, so a file in
/// that slot would be written and then never read.
pub(crate) fn save_preset(
    name: &str,
    base_url: &Option<String>,
    models: &ModelSettings,
) -> Result<()> {
    let trimmed = name.trim();
    if is_builtin(trimmed) {
        bail!("'{trimmed}' is a built-in preset and cannot be overwritten");
    }
    let path = preset_path(trimmed)?;
    let body = serde_json::to_string_pretty(&PresetFile {
        base_url: base_url.clone(),
        models: models.clone(),
    })?;
    // `atomic_write_600` creates a missing parent 0o700 itself, so the dir and
    // the file are both born owner-only.
    atomic_write_600(&path, format!("{body}\n"))?;
    Ok(())
}

pub(crate) fn delete_preset(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if is_builtin(trimmed) {
        bail!("'{trimmed}' is a built-in preset and cannot be deleted");
    }
    let path = preset_path(trimmed)?;
    if !path.exists() {
        bail!("no preset named '{trimmed}'");
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/inline/presets.rs"]
mod tests;
