param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$Arguments = "",
    [int]$WarmupSeconds = 5,
    [int]$Seconds = 30
)

# Замер памяти (рабочий набор, закрытая память) и процессора процесса и всех его потомков.

function Get-Tree([int]$RootId) {
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId
    $ids = New-Object System.Collections.Generic.List[int]
    $ids.Add($RootId)
    for ($i = 0; $i -lt $ids.Count; $i++) {
        foreach ($c in $all) {
            if ($c.ParentProcessId -eq $ids[$i] -and -not $ids.Contains([int]$c.ProcessId)) {
                $ids.Add([int]$c.ProcessId)
            }
        }
    }
    return $ids
}

function Get-Snapshot([int]$RootId) {
    $ws = 0.0; $priv = 0.0; $cpu = 0.0
    foreach ($id in (Get-Tree $RootId)) {
        $p = Get-Process -Id $id -ErrorAction SilentlyContinue
        if ($p) {
            $ws += $p.WorkingSet64
            $priv += $p.PrivateMemorySize64
            $cpu += $p.TotalProcessorTime.TotalSeconds
        }
    }
    return [pscustomobject]@{ Ws = $ws; Priv = $priv; Cpu = $cpu }
}

$startArgs = @{ FilePath = $Exe; PassThru = $true }
if ($Arguments -ne "") { $startArgs.ArgumentList = $Arguments }
$proc = Start-Process @startArgs

try {
    Start-Sleep -Seconds $WarmupSeconds
    $first = Get-Snapshot $proc.Id
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $wsSum = 0.0; $privSum = 0.0; $n = 0
    for ($i = 0; $i -lt $Seconds; $i++) {
        Start-Sleep -Seconds 1
        $s = Get-Snapshot $proc.Id
        $wsSum += $s.Ws
        $privSum += $s.Priv
        $n++
    }
    $clock.Stop()
    $last = Get-Snapshot $proc.Id
    $cpuPct = 0.0
    if ($clock.Elapsed.TotalSeconds -gt 0) {
        $cpuPct = ($last.Cpu - $first.Cpu) / $clock.Elapsed.TotalSeconds * 100.0
    }
    $result = [ordered]@{
        workingSetMB      = [math]::Round($wsSum / $n / 1MB, 1)
        privateMB         = [math]::Round($privSum / $n / 1MB, 1)
        cpuPercentOneCore = [math]::Round([math]::Max($cpuPct, 0.0), 2)
        samples           = $n
    }
    $result | ConvertTo-Json -Compress
}
finally {
    foreach ($id in (Get-Tree $proc.Id)) {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }
}
