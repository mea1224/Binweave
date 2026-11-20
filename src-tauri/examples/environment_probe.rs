use binweave_lib::mcp_probe;

#[tokio::main]
async fn main() {
    let workspace = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("Workspace unavailable"));
    if !workspace.is_dir() {
        eprintln!("Workspace must be an existing directory");
        std::process::exit(2);
    }
    let report = mcp_probe::inspect_environment(&workspace).await;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("Unable to encode report")
    );
}
