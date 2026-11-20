[CmdletBinding()]
param(
    [string]$Command = "idalib-mcp",
    [string[]]$CommandArgs = @("--stdio", "--max-workers", "1"),
    [int]$InitWaitSec = 15,
    [int]$AfterListWaitSec = 15,
    [int]$AfterCallWaitSec = 15,
    [int]$HeartbeatSec = 5,
    [string]$ToolName = "read_bytes",
    [string]$Address = "0x140001000",
    [int]$Size = 16
)

Set-StrictMode -Off
$ErrorActionPreference = "Continue"

$startTime = Get-Date
function Now-Stamp() {
    $now = Get-Date
    $elapsed = ($now - $startTime).TotalSeconds
    return "[{0:HH:mm:ss.fff} | +{1:0.0}s]" -f $now, $elapsed
}

$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName = $Command
$psi.Arguments = ($CommandArgs -join " ")
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.CreateNoWindow = $true
$psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
$psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8

$proc = [System.Diagnostics.Process]::new()
$proc.StartInfo = $psi

$outTask = $null
$errTask = $null

try {
    Write-Host "$(Now-Stamp) [INFO] 启动进程: $Command $($psi.Arguments)" -ForegroundColor Cyan
    if (-not $proc.Start()) {
        throw "无法启动进程: $Command"
    }
    Write-Host "$(Now-Stamp) [INFO] PID=$($proc.Id) 已启动" -ForegroundColor Cyan

    $outTask = [System.Threading.Tasks.Task]::Run([Action]{
        try {
            while (-not $proc.StandardOutput.EndOfStream) {
                $line = $proc.StandardOutput.ReadLine()
                if ($null -ne $line) {
                    Write-Host "$(Now-Stamp) [STDOUT] $line" -ForegroundColor Green
                }
            }
        } catch {}
    })

    $errTask = [System.Threading.Tasks.Task]::Run([Action]{
        try {
            while (-not $proc.StandardError.EndOfStream) {
                $line = $proc.StandardError.ReadLine()
                if ($null -ne $line) {
                    Write-Host "$(Now-Stamp) [STDERR] $line" -ForegroundColor Yellow
                }
            }
        } catch {}
    })

    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    $writer = [System.IO.StreamWriter]::new($proc.StandardInput.BaseStream, $utf8NoBom)
    $writer.AutoFlush = $true

    function Send-Rpc([string]$JsonLine) {
        Write-Host "$(Now-Stamp) [SEND]   $JsonLine" -ForegroundColor Cyan
        $writer.WriteLine($JsonLine)
    }

    function Wait-With-Heartbeat([int]$Seconds, [string]$PhaseName) {
        $deadline = (Get-Date).AddSeconds($Seconds)
        $nextBeat = (Get-Date).AddSeconds($HeartbeatSec)
        while ((Get-Date) -lt $deadline) {
            if ($proc.HasExited) {
                Write-Host "$(Now-Stamp) [WARN] ($PhaseName) 进程已退出，ExitCode=$($proc.ExitCode)" -ForegroundColor Red
                return $false
            }
            if ((Get-Date) -ge $nextBeat) {
                Write-Host "$(Now-Stamp) [HEARTBEAT] ($PhaseName) 仍在等待中... HasExited=$($proc.HasExited)" -ForegroundColor DarkGray
                $nextBeat = (Get-Date).AddSeconds($HeartbeatSec)
            }
            Start-Sleep -Milliseconds 200
        }
        return (-not $proc.HasExited)
    }

    # 1. initialize
    Send-Rpc '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe-timed","version":"0.1.0"}}}'
    if (-not (Wait-With-Heartbeat -Seconds $InitWaitSec -PhaseName "initialize")) { return }

    # 2. notifications/initialized
    Send-Rpc '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    Start-Sleep -Milliseconds 300

    # 3. tools/list
    Send-Rpc '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
    if (-not (Wait-With-Heartbeat -Seconds $AfterListWaitSec -PhaseName "tools/list")) { return }

    # 4. tools/call
    $callPayload = @{
        jsonrpc = "2.0"
        id      = 3
        method  = "tools/call"
        params  = @{
            name      = $ToolName
            arguments = @{
                address = $Address
                size    = $Size
            }
        }
    } | ConvertTo-Json -Depth 10 -Compress

    Send-Rpc $callPayload
    $null = Wait-With-Heartbeat -Seconds $AfterCallWaitSec -PhaseName "tools/call:$ToolName"

    Write-Host "$(Now-Stamp) [INFO] 探测窗口结束，最终 HasExited=$($proc.HasExited)" -ForegroundColor Cyan
    if ($proc.HasExited) {
        Write-Host "$(Now-Stamp) [INFO] 最终 ExitCode=$($proc.ExitCode)" -ForegroundColor Red
    }
}
catch {
    Write-Host "$(Now-Stamp) [ERR]  脚本异常: $($_.Exception.Message)" -ForegroundColor Red
}
finally {
    try { if ($writer) { $writer.Close() } } catch {}
    try {
        if ($proc -and -not $proc.HasExited) {
            Write-Host "$(Now-Stamp) [INFO] 清理残留探测进程 PID=$($proc.Id)" -ForegroundColor DarkGray
            $proc.Kill()
        }
    } catch {}
    try { if ($proc) { $proc.Dispose() } } catch {}
}