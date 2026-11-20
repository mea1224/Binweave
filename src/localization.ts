// Presentation-only translations of application-owned backend messages.
// Persisted evidence, notes, protocol identifiers, paths and artifacts stay intact.
const messages: Record<string, string> = {
  'Review diagnostics and role contract': '请检查环境诊断和角色契约。',
  'Workspace initialized. No agent has been launched.': '工作区已初始化，尚未启动任何智能体。',
  'Workspace changed by the operator; diagnostics must be rerun.': '操作员已更改工作区，请重新运行环境诊断。',
  'Analyst and implementer roles swapped by the operator.': '操作员已交换分析员与实现员的角色。',
  'Operator paused the idle run. No worker is running.': '操作员已暂停空闲任务，当前没有工作进程运行。',
  'Operator reviewed the checkpoint. Ready for a new instruction; no command was replayed.': '操作员已确认检查点，可以接收新指令；未重放任何命令。',
  'REA availability': 'REA 可用性',
  'Codex MCP configuration': 'Codex MCP 配置',
  'Claude MCP configuration': 'Claude MCP 配置',
  'REA is available through a verified MCP server; a PATH alias is not required.': '可通过已验证的 MCP 服务使用 REA，无需配置 PATH 别名。',
  'REA command found. MCP communication is not verified.': '已找到 REA 命令，尚未验证 MCP 通信。',
  'No REA executable on PATH and no verified MCP connection.': 'PATH 中未找到 REA 可执行文件，且没有已验证的 MCP 连接。',
  'Configuration parsed. Credentials are never included in the report.': '配置已解析，报告不包含任何凭据。',
  'No REA MCP entry found for this agent.': '未找到该智能体的 REA MCP 配置项。',
  'Configuration is unavailable or missing': '配置不可用或不存在。',
  'Configuration exceeds the 8 MiB safety limit': '配置文件超过 8 MiB 安全限制。',
  'Unable to read configuration': '无法读取配置。',
  'Invalid TOML configuration (contents omitted)': 'TOML 配置无效（已隐藏配置内容）。',
  'Unable to parse TOML configuration': '无法解析 TOML 配置。',
  'Invalid JSON configuration (contents omitted)': 'JSON 配置无效（已隐藏配置内容）。',
  'Server is disabled in configuration.': '配置中已禁用该服务。',
  'Configured remote transport detected. HTTP/SSE probing is not implemented in this foundation release.': '检测到远程传输配置，基础版尚未实现 HTTP/SSE 探测。',
  'Project MCP command detected but not executed. Explicit project trust is required.': '检测到项目 MCP 命令，但未执行；需先明确信任该项目。',
  'Configured stdio server; probing protocol.': '已配置 stdio 服务，正在探测协议。',
  'MCP command is missing': '缺少 MCP 命令。',
  'MCP args must be an array of strings': 'MCP 参数必须为字符串数组。',
  'MCP environment values must be strings': 'MCP 环境变量值必须为字符串。',
  'Unresolved environment substitution in MCP configuration': 'MCP 配置中存在未解析的环境变量替换。',
  'Configured MCP working directory does not exist': '配置的 MCP 工作目录不存在。',
  'Unable to launch configured MCP executable': '无法启动配置的 MCP 可执行文件。',
  'MCP stdin is unavailable': 'MCP 标准输入不可用。',
  'MCP stdout is unavailable': 'MCP 标准输出不可用。',
  'MCP stderr is unavailable': 'MCP 标准错误不可用。',
  'MCP handshake timed out after 10 seconds': 'MCP 握手超时（10 秒）。',
  'Unable to encode MCP request': '无法编码 MCP 请求。',
  'MCP stdin closed': 'MCP 标准输入已关闭。',
  'MCP stdin flush failed': 'MCP 标准输入刷新失败。',
  'MCP server closed stdout before responding': 'MCP 服务在响应前关闭了标准输出。',
  'MCP stdout contains invalid JSON': 'MCP 标准输出包含无效的 JSON。',
  'MCP message is not JSON-RPC 2.0': 'MCP 消息不符合 JSON-RPC 2.0。',
  'MCP response has no result': 'MCP 响应缺少结果。',
  'Unsupported negotiated MCP protocol version': '不支持协商得到的 MCP 协议版本。',
  'Invalid server information or missing tools capability': '服务信息无效或缺少工具能力声明。',
  'MCP tools/list omitted the tools array': 'MCP tools/list 响应缺少工具数组。',
  'MCP tool has no name': 'MCP 工具缺少名称。',
  'MCP tool has no input schema': 'MCP 工具缺少输入结构定义。',
  'MCP tools/list exceeded the 16-page diagnostic limit': 'MCP tools/list 超过 16 页诊断限制。',
  'CLI shim has no parent directory': 'CLI 启动脚本缺少父目录。',
  'Unsupported Windows shell shim. Configure the native executable or Node script explicitly.': '不支持此 Windows 启动脚本，请明确配置原生可执行文件或 Node 脚本。',
  'Executable not found or not executable': '未找到可执行文件，或文件无法执行。',
  'Unable to read process output': '无法读取进程输出。',
  'Process output exceeded the diagnostic limit': '进程输出超过诊断限制。',
  'Unable to start executable': '无法启动可执行文件。',
  'Missing stdout pipe': '缺少标准输出管道。',
  'Missing stderr pipe': '缺少标准错误管道。',
  'Unable to wait for process': '等待进程结束时出错。',
  'Executable returned a failure status': '可执行文件返回失败状态。',
  'Executable timed out': '可执行文件运行超时。',
  'Unable to read process stream': '无法读取进程数据流。',
  'Process frame exceeds the configured size limit': '进程数据帧超过配置的大小限制。',
  'Process stream is not valid UTF-8': '进程数据流不是有效的 UTF-8。',
};

const states: Record<string, string> = {
  Ready: '就绪', Analyzing: '分析中', Implementing: '实现中', Verifying: '验证中',
  UpdatingSkill: '更新技能中', Paused: '已暂停', Interrupted: '已中断', Completed: '已完成', Failed: '失败',
};

export function systemText(value: string, depth = 0): string {
  if (Object.hasOwn(messages, value)) return messages[value];
  if (depth >= 8) return value;
  const transition = /^(Ready|Analyzing|Implementing|Verifying|UpdatingSkill|Paused|Interrupted|Completed|Failed) -> (Ready|Analyzing|Implementing|Verifying|UpdatingSkill|Paused|Interrupted|Completed|Failed): ([\s\S]*)$/.exec(value);
  if (transition) return `${states[transition[1]]} → ${states[transition[2]]}：${systemText(transition[3], depth + 1)}`;
  const interruption = /^Interrupted during (Ready|Analyzing|Implementing|Verifying|UpdatingSkill|Paused|Interrupted|Completed|Failed): ([\s\S]*)\. Review before resuming; no work has been replayed\.$/.exec(value);
  if (interruption) return `任务在“${states[interruption[1]]}”阶段中断：${systemText(interruption[2], depth + 1)} 恢复前请检查，未重放任何操作。`;
  const artifact = /^Role contract v(\d+) saved: ([\s\S]+)$/.exec(value);
  if (artifact) return `角色契约 v${artifact[1]} 已保存：${artifact[2]}`;
  const version = /^Version check passed: ([\s\S]*)\. Authentication and model access are not verified\.$/.exec(value);
  if (version) return `版本检查通过：${version[1] === 'unknown' ? '未知版本' : version[1]}。尚未验证身份认证和模型访问权限。`;
  const tools = /^initialize and tools\/list succeeded: (\d+) tools\. No analysis tools were executed\.$/.exec(value);
  if (tools) return `initialize 和 tools/list 成功，共 ${tools[1]} 个工具；未执行任何分析工具。`;
  const rpc = /^MCP request failed with JSON-RPC code (-?\d+)$/.exec(value);
  if (rpc) return `MCP 请求失败，JSON-RPC 错误码：${rpc[1]}。`;
  return value;
}

export function serverName(value: string): string {
  return value.replace(/\((Codex|Claude), (global|project)\)$/, (_, agent: string, scope: string) => `（${agent}，${scope === 'global' ? '全局' : '项目'}）`);
}

export function transportName(value: string): string {
  return value === 'stdio' ? '标准输入输出（stdio）' : value === 'unknown' || value === 'none' ? '未配置' : value;
}