//! Workspace role charters are operator-owned. Only missing files are initialized.
use crate::model::{LogicalRole, RoleSkill};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const LIMIT: u64 = 65_536;

fn reject_link(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            let linked = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0 // reparse points, including junctions
            };
            #[cfg(not(windows))]
            let linked = metadata.file_type().is_symlink();
            if linked {
                return Err(format!(
                    "角色技能路径不能使用符号链接或目录联接：{}",
                    path.display()
                ));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("无法检查角色技能路径 {}：{e}", path.display())),
    }
}

fn template(role: LogicalRole) -> String {
    if role == LogicalRole::Analyst {
        return include_str!("../../.rule/analyst/SKILL.md").into();
    }
    let rules = match role {
        LogicalRole::Analyst => "使用 REA / REA MCP 查证符号、结构、偏移、协议和状态边界。区分已验证事实与假设，并给出证据。回答实现方的每个查证问题，维护实现方任务技能。不直接编写或修改项目功能源码。任务技能通过本轮 skillUpdate 提交，由编排器保存版本；不得修改静态角色契约。REA 不可用或证据不足时明确报告并停留在查证阶段，不编造结果。",
        LogicalRole::Implementer => "依据分析结论和最新任务技能编写、测试项目代码。遇到结构、偏移、协议、状态或边界不确定，立即停止后续编写，输出 NEED_REA_QUERY 或 BLOCKED_LOGIC，包含 stopped_at、what_is_clear 和 questions（id、target、question、why_needed）。收到分析方逐项查证结果后才能继续。不得猜测缺失事实，不得篡改角色技能。仅报告实际执行过的构建与测试。",
    };
    format!("---\nname: binweave-{}\ndescription: Binweave 逻辑角色技能\n---\n\n# {}\n\n{rules}\n\n角色与 CLI 无关，每轮以编排器注入的 logical_role、provider 和 turnId 为准。结构化交接遵守本轮调度协议；旧 turnId 无效。\n", role.name(), role.name())
}

fn directory(workspace: &Path, role: LogicalRole) -> Result<PathBuf, String> {
    if !workspace.is_absolute() || !workspace.is_dir() {
        return Err("角色技能需要有效的工作区绝对路径".into());
    }
    let root = workspace.canonicalize().map_err(|e| e.to_string())?;
    let mut path = root.clone();
    for part in [".rule", role.name()] {
        path.push(part);
        reject_link(&path)?;
        match fs::create_dir(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(format!("无法创建角色技能目录 {}：{e}", path.display())),
        }
        if !path.is_dir()
            || !path
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(&root)
        {
            return Err(format!(
                "角色技能目录超出工作区或不是目录：{}",
                path.display()
            ));
        }
    }
    Ok(path)
}

pub fn load(workspace: &Path, role: LogicalRole) -> Result<RoleSkill, String> {
    let path = directory(workspace, role)?.join("SKILL.md");
    reject_link(&path)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => file
            .write_all(template(role).as_bytes())
            .map_err(|e| format!("角色技能写入失败：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(format!("无法初始化角色技能 {}：{e}", path.display())),
    }
    let resolved = path
        .canonicalize()
        .map_err(|e| format!("无法解析角色技能 {}：{e}", path.display()))?;
    if !resolved.starts_with(workspace.canonicalize().map_err(|e| e.to_string())?)
        || !resolved.is_file()
    {
        return Err("角色技能文件必须位于当前工作区内".into());
    }
    let mut content = String::new();
    fs::File::open(&resolved)
        .map_err(|e| e.to_string())?
        .take(LIMIT + 1)
        .read_to_string(&mut content)
        .map_err(|e| format!("无法读取 UTF-8 角色技能 {}：{e}", path.display()))?;
    if content.trim().is_empty() || content.len() as u64 > LIMIT {
        return Err(format!(
            "角色技能需包含 1–65536 字节的非空内容：{}",
            path.display()
        ));
    }
    Ok(RoleSkill {
        path: path.to_string_lossy().into_owned(),
        content,
    })
}

pub fn initialize(workspace: &Path) -> Result<(), String> {
    for role in [LogicalRole::Analyst, LogicalRole::Implementer] {
        load(workspace, role)?;
    }
    Ok(())
}

/// Immutable, uniquely named archive. SQLite remains the authority for which
/// revision is active; failed transactions never activate an orphaned file.
pub fn archive_task(workspace: &Path, artifact_id: &str, content: &str) -> Result<String, String> {
    uuid::Uuid::parse_str(artifact_id).map_err(|_| "任务技能标识无效")?;
    let path =
        directory(workspace, LogicalRole::Implementer)?.join(format!("generated_{artifact_id}.md"));
    reject_link(&path)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => file
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut saved = String::new();
            fs::File::open(&path)
                .map_err(|e| e.to_string())?
                .take(LIMIT + 1)
                .read_to_string(&mut saved)
                .map_err(|e| e.to_string())?;
            if saved != content {
                return Err("任务技能归档内容冲突".into());
            }
        }
        Err(e) => return Err(format!("任务技能归档失败：{e}")),
    }
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_paths_utf8_and_archive_conflicts() {
        assert!(load(Path::new("relative"), LogicalRole::Analyst).is_err());
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".rule"), "not a directory").unwrap();
        assert!(initialize(dir.path()).is_err());
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        fs::write(dir.path().join(".rule/analyst/SKILL.md"), [255, 254]).unwrap();
        assert!(load(dir.path(), LogicalRole::Analyst).is_err());
        let id = uuid::Uuid::new_v4().to_string();
        let path = archive_task(dir.path(), &id, "任务技能").unwrap();
        archive_task(dir.path(), &id, "任务技能").unwrap();
        assert!(archive_task(dir.path(), &id, "不同内容").is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "任务技能");
        assert!(archive_task(dir.path(), "../outside", "内容").is_err());
    }
    #[test]
    fn initializes_preserves_and_reloads_operator_skills() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        let skill = load(dir.path(), LogicalRole::Analyst).unwrap();
        assert!(skill.content.contains("REA / REA MCP"));
        fs::write(&skill.path, "用户自定义分析技能").unwrap();
        initialize(dir.path()).unwrap();
        assert_eq!(
            load(dir.path(), LogicalRole::Analyst).unwrap().content,
            "用户自定义分析技能"
        );
        assert!(load(dir.path(), LogicalRole::Implementer)
            .unwrap()
            .content
            .contains("立即停止"));
        for invalid in [String::new(), "x".repeat(65_537)] {
            fs::write(&skill.path, invalid).unwrap();
            assert!(load(dir.path(), LogicalRole::Analyst).is_err());
        }
    }
}
