use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub const BUNDLED_SKILL_NAME: &str = "nictui-radio-cli";

const BUNDLED_SKILL_MD: &str = include_str!("../skills/nictui-radio-cli/SKILL.md");
const BUNDLED_OPENAI_YAML: &str = include_str!("../skills/nictui-radio-cli/agents/openai.yaml");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillInstallTarget {
    Auto,
    Codex,
    Claude,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedAgent {
    Codex,
    Claude,
}

#[derive(Debug, Clone)]
pub struct SkillInstallResult {
    pub agent: SupportedAgent,
    pub skill_dir: PathBuf,
    pub changed: bool,
}

pub fn bundled_skill_markdown() -> &'static str {
    BUNDLED_SKILL_MD
}

pub fn install_bundled_skill(target: SkillInstallTarget) -> Result<Vec<SkillInstallResult>> {
    let targets = resolve_install_targets(target)?;
    let mut results = Vec::with_capacity(targets.len());

    for agent in targets {
        let skill_dir = skill_dir_for_agent(agent)?;
        let changed = write_skill_tree(&skill_dir)?;
        results.push(SkillInstallResult {
            agent,
            skill_dir,
            changed,
        });
    }

    Ok(results)
}

pub fn print_post_exit_skill_hint() {
    if !std::io::stdout().is_terminal() && !std::io::stderr().is_terminal() {
        return;
    }

    let detected = detected_agents();
    if detected.is_empty() {
        return;
    }

    let agents = detected
        .iter()
        .map(|agent| agent.display_name())
        .collect::<Vec<_>>()
        .join(" and ");

    eprintln!();
    eprintln!(
        "Hint: detected {}. Install the bundled NicTUI AI skill with `nictui skill install`.",
        agents
    );
    eprintln!(
        "The skill teaches Codex and Claude Code to modify the radio through the nictui CLI."
    );
}

pub fn detected_agents() -> Vec<SupportedAgent> {
    let mut agents = Vec::new();
    if command_exists("codex") {
        agents.push(SupportedAgent::Codex);
    }
    if command_exists("claude") {
        agents.push(SupportedAgent::Claude);
    }
    agents
}

impl SupportedAgent {
    pub const fn all() -> [SupportedAgent; 2] {
        [SupportedAgent::Codex, SupportedAgent::Claude]
    }

    pub fn display_name(self) -> &'static str {
        match self {
            SupportedAgent::Codex => "Codex",
            SupportedAgent::Claude => "Claude Code",
        }
    }
}

pub fn bundled_skill_dir(agent: SupportedAgent) -> Result<PathBuf> {
    skill_dir_for_agent(agent)
}

fn resolve_install_targets(target: SkillInstallTarget) -> Result<Vec<SupportedAgent>> {
    let detected = detected_agents();
    let targets = match target {
        SkillInstallTarget::Auto => detected,
        SkillInstallTarget::Codex => vec![SupportedAgent::Codex],
        SkillInstallTarget::Claude => vec![SupportedAgent::Claude],
        SkillInstallTarget::All => vec![SupportedAgent::Codex, SupportedAgent::Claude],
    };

    if targets.is_empty() {
        bail!(
            "No supported AI agents were detected on PATH. Install Codex or Claude Code first, or rerun with --agent codex / --agent claude."
        );
    }

    Ok(targets)
}

fn skill_dir_for_agent(agent: SupportedAgent) -> Result<PathBuf> {
    let root = match agent {
        SupportedAgent::Codex => {
            if let Some(codex_home) = env::var_os("CODEX_HOME") {
                PathBuf::from(codex_home)
            } else {
                home_dir()?.join(".codex")
            }
        }
        SupportedAgent::Claude => home_dir()?.join(".claude"),
    };

    Ok(root.join("skills").join(BUNDLED_SKILL_NAME))
}

fn write_skill_tree(skill_dir: &Path) -> Result<bool> {
    fs::create_dir_all(skill_dir.join("agents"))
        .with_context(|| format!("Failed to create {}", skill_dir.display()))?;

    let mut changed = false;
    changed |= write_if_changed(&skill_dir.join("SKILL.md"), BUNDLED_SKILL_MD)?;
    changed |= write_if_changed(
        &skill_dir.join("agents").join("openai.yaml"),
        BUNDLED_OPENAI_YAML,
    )?;
    Ok(changed)
}

fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    match fs::read_to_string(path) {
        Ok(existing) if existing == content => Ok(false),
        Ok(_) | Err(_) => {
            fs::write(path, content)
                .with_context(|| format!("Failed to write {}", path.display()))?;
            Ok(true)
        }
    }
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn command_exists(command: &str) -> bool {
    let Some(path_value) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&path_value).any(|dir| executable_exists(&dir.join(command)))
}

fn executable_exists(path: &Path) -> bool {
    if path.is_file() {
        return true;
    }

    #[cfg(windows)]
    {
        for extension in ["exe", "bat", "cmd"] {
            if path.with_extension(extension).is_file() {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::bundled_skill_markdown;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn bundled_skill_mentions_safe_write_and_ble_workflows() {
        let skill = bundled_skill_markdown();
        assert!(skill.contains("Safe write workflow"));
        assert!(skill.contains("--validate-only"));
        assert!(skill.contains("nictui bluetooth on"));
        assert!(skill.contains("ble://<uuid>"));
        assert!(skill.contains("nictui remote key"));
        assert!(skill.contains("0000ff00-0000-1000-8000-00805f9b34fb"));
        assert!(skill.contains(
            "doctor \"${NICTUI_TARGET[@]}\" --output-dir nictui-radio-session --json --codeplug"
        ));
    }

    #[test]
    fn plugin_skill_copy_matches_bundled_skill() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let plugin_skill =
            manifest_dir.join("plugins/nictui-radio-cli-plugin/skills/nictui-radio-cli/SKILL.md");
        let plugin_contents = fs::read_to_string(plugin_skill).unwrap();
        assert_eq!(plugin_contents, bundled_skill_markdown());
    }
}
