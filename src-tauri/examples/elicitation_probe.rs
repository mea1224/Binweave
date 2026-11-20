//! Isolated, read-only real Codex/REA transport probe. Explicitly cancels MCP
//! requests; never treats missing elicitation as a successful elicitation test.
use binweave_lib::{model, process};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[tokio::main]
async fn main() -> Result<(), String> {
    let cwd = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .ok_or("请提供工作区绝对路径")?;
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err("工作区路径无效".into());
    }
    let config = process::interactive::Config {
        analyst: true,
        broker: Arc::new(process::interactive::Broker::default()),
        run_id: uuid::Uuid::new_v4().to_string(),
        prompt_id: uuid::Uuid::new_v4().to_string(),
        session: None,
        elicitation_policy: model::ElicitationPolicy::Cancel,
    };
    let spec = process::stream::RunSpec {
        agent: model::AgentId::Codex, executable: "codex".into(), args: vec!["app-server".into()],
        cwd, env: HashMap::new(), interactive: None, deadline: Duration::from_secs(50),
        input: Some("这是 Binweave MCP 交互兼容性测试。仅调用 REA 的 list_documents 一次，不打开文件、不运行命令、不修改任何内容。若需要用户确认，等待协议回复；若用户取消，停止尝试并说明结果。不要调用其他工具。".into()),
    };
    let (_stop, cancel) = tokio::sync::oneshot::channel();
    let delivered = Mutex::new(false);
    let result = process::interactive::run(&spec, &config, cancel, |output| {
        if let Some(r) = output.interaction {
            println!(
                "交互类型={} 模式={:?} 状态={} 投递={:?}",
                r.kind, r.mcp_mode, r.status, r.delivery
            );
            if r.kind.starts_with("mcp_") && r.delivery.as_deref() == Some("delivered") {
                *delivered.lock().unwrap() = true;
            }
        }
        Ok(())
    })
    .await;
    println!("Codex 执行结果：{result:?}");
    if !*delivered.lock().unwrap() {
        return Err(
            "本次未观测到真实 MCP elicitation 响应投递，不能记为端到端交互验收通过。".into(),
        );
    }
    if result != Ok(0) {
        return Err("已投递 MCP 响应，但后续轮次未正常结束。".into());
    }
    println!("真实 Codex/REA MCP 取消响应已投递，原轮次正常结束。");
    Ok(())
}
