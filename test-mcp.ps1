[CmdletBinding()]
param(
    [string]$Command = "idalib-mcp",
    [string[]]$CommandArgs = @("--stdio", "--max-workers", "1"),
    [string]$ToolName = "read_bytes",
    [string]$Address = "0x140001000",
    [int]$Size = 16,
    [string]$ExtraArgsJson = "{}",
    [int]$TimeoutSec = 20
)

Set-StrictMode -Off
$ErrorActionPreference = "Continue"

$outQueue = [System.Collections.Concurrent.ConcurrentQueue[string]]::new()
$errQueue = [System.Collections.Concurrent.ConcurrentQueue[string]]::new()

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

try {
    Write-Host "[INFO] 启动: $Command $($psi.Arguments)" -ForegroundColor Cyan
    if (-not $proc.Start()) {
        throw "无法启动进程: $Command"
    }

    # 后台线程持续读取 stdout / stderr，避免管道缓冲区满阻塞
    $outJob = Start-Job -ScriptBlock {
        param($handle)
    } -ArgumentList 1 -ErrorAction SilentlyContinue
    if ($outJob) { Remove-Job $outJob -Force -ErrorAction SilentlyContinue }

    $outReaderTask = [System.Threading.Tasks.Task]::Run([Action]{
        try {
            while (-not $proc.StandardOutput.EndOfStream) {
                $line = $proc.StandardOutput.ReadLine()
                if ($null -ne $line) { $outQueue.Enqueue($line) }
            }
        } catch {}
    })

    $errReaderTask = [System.Threading.Tasks.Task]::Run([Action]{
        try {
            while (-not $proc.StandardError.EndOfStream) {
                $line = $proc.StandardError.ReadLine()
                if ($null -ne $line) { $errQueue.Enqueue($line) }
            }
        } catch {}
    })

    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    $writer = [System.IO.StreamWriter]::new($proc.StandardInput.BaseStream, $utf8NoBom)
    $writer.AutoFlush = $true

    function Send-Rpc([hashtable]$Payload) {
        $json = $Payload | ConvertTo-Json -Depth 15 -Compress
        $writer.WriteLine($json)
    }

    function Wait-For-Id([int]$TargetId, [int]$Sec) {
        $deadline = (Get-Date).AddSeconds($Sec)
        $seen = @{}
        while ((Get-Date) -lt $deadline) {
            if ($proc.HasExited) {
                Write-Host "[WARN] 进程已提前退出，退出码: $($proc.ExitCode)" -ForegroundColor Yellow
                return $null
            }
            foreach ($item in $outQueue.ToArray()) {
                if ($seen.ContainsKey($item)) { continue }
                $seen[$item] = $true
                try {
                    $obj = $item | ConvertFrom-Json -ErrorAction Stop
                    if ($null -ne $obj.id -and [int]$obj.id -eq $TargetId) {
                        return $item
                    }
                } catch {}
            }
            Start-Sleep -Milliseconds 150
        }
        return $null
    }

    Write-Host "[INFO] 1/4 发送 initialize" -ForegroundColor Cyan
    Send-Rpc @{
        jsonrpc = "2.0"
        id      = 1
        method  = "initialize"
        params  = @{
            protocolVersion = "2024-11-05"
            capabilities    = @{}
            clientInfo      = @{ name = "probe"; version = "0.1.0" }
        }
    }
    Start-Sleep -Milliseconds 300

    Write-Host "[INFO] 2/4 发送 notifications/initialized" -ForegroundColor Cyan
    Send-Rpc @{
        jsonrpc = "2.0"
        method  = "notifications/initialized"
    }
    Start-Sleep -Milliseconds 200

    Write-Host "[INFO] 3/4 发送 tools/list (id=2)" -ForegroundColor Cyan
    Send-Rpc @{
        jsonrpc = "2.0"
        id      = 2
        method  = "tools/list"
        params  = @{}
    }
    $listResp = Wait-For-Id -TargetId 2 -Sec $TimeoutSec
    if ($listResp) {
        Write-Host "[OK]   收到 tools/list:" -ForegroundColor Green
        Write-Host $listResp
    } else {
        Write-Host "[WARN] 未收到 id=2 的 tools/list 响应" -ForegroundColor Yellow
    }

    $argsMap = @{}
    if (-not [string]::IsNullOrWhiteSpace($ExtraArgsJson) -and $ExtraArgsJson.Trim() -ne "{}") {
        try {
            $parsedExtra = $ExtraArgsJson | ConvertFrom-Json -AsHashtable -ErrorAction Stop
            foreach ($k in $parsedExtra.Keys) { $argsMap[$k] = $parsedExtra[$k] }
        } catch {
            Write-Host "[WARN] ExtraArgsJson 解析失败，使用默认参数: $($_.Exception.Message)" -ForegroundColor Yellow
        }
    }
    if (-not $argsMap.ContainsKey("address") -and -not $argsMap.ContainsKey("ea")) {
        $argsMap["address"] = $Address
    }
    if (-not $argsMap.ContainsKey("size") -and -not $argsMap.ContainsKey("length")) {
        $argsMap["size"] = $Size
    }

    Write-Host "[INFO] 4/4 发送 tools/call $ToolName (id=3)" -ForegroundColor Cyan
    Send-Rpc @{
        jsonrpc = "2.0"
        id      = 3
        method  = "tools/call"
        params  = @{
            name      = $ToolName
            arguments = $argsMap
        }
    }
    $callResp = Wait-For-Id -TargetId 3 -Sec $TimeoutSec
    if ($callResp) {
        Write-Host "[OK]   收到 tools/call 响应:" -ForegroundColor Green
        Write-Host $callResp
    } else {
        Write-Host "[WARN] 未收到 id=3 的 tools/call 响应" -ForegroundColor Yellow
    }

    try { $writer.Close() } catch {}
    Start-Sleep -Milliseconds 300

    if (-not $errQueue.IsEmpty) {
        Write-Host "[WARN] STDERR:" -ForegroundColor Yellow
        $errQueue.ToArray() | ForEach-Object { Write-Host $_ -ForegroundColor DarkYellow }
    }
    if (-not $outQueue.IsEmpty) {
        Write-Host "[INFO] 全部 STDOUT 行:" -ForegroundColor Cyan
        $outQueue.ToArray() | ForEach-Object { Write-Host $_ }
    }
}
catch {
    Write-Host "[ERR]  脚本异常: $($_.Exception.Message)" -ForegroundColor Red
    if ($_.ScriptStackTrace) {
        Write-Host $_.ScriptStackTrace -ForegroundColor DarkRed
    }
}
finally {
    try {
        if ($proc -and -not $proc.HasExited) { $proc.Kill() }
    } catch {}
    if ($proc) { $proc.Dispose() }
}