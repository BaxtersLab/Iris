param()

# CI smoke test: start iris-ui, call /debug/force_rebase, assert metric==1
try {
    Write-Output "Starting iris-ui in background..."
    $proc = Start-Process -FilePath cargo -ArgumentList 'run','-p','iris-ui','--quiet' -PassThru

    # wait for server to come up
    $ok = $false
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        try {
            $res = Invoke-WebRequest -Uri http://127.0.0.1:9180/metrics -UseBasicParsing -TimeoutSec 2 -ErrorAction Stop
            if ($res.StatusCode -eq 200) { $ok = $true; break }
        } catch { Start-Sleep -Seconds 1 }
    }

    if (-not $ok) { throw "Metrics endpoint did not become available" }

    Write-Output "Triggering /debug/force_rebase..."
    Invoke-WebRequest -Uri http://127.0.0.1:9180/debug/force_rebase -UseBasicParsing -TimeoutSec 5 -ErrorAction Stop | Out-Null
    Start-Sleep -Seconds 1

    Write-Output "Fetching /metrics..."
    $metrics = Invoke-WebRequest -Uri http://127.0.0.1:9180/metrics -UseBasicParsing -TimeoutSec 5 -ErrorAction Stop
    $text = $metrics.Content

    Write-Output "Metrics content:`n$text"

    # parse the iris_encoder_rebase_total value
    $match = Select-String -InputObject $text -Pattern 'iris_encoder_rebase_total\{[^}]*\}\s*(\d+)' -AllMatches
    if (-not $match) { throw "Metric iris_encoder_rebase_total not found" }
    $val = [int]$match.Matches[0].Groups[1].Value
    if ($val -ne 1) { throw "Expected iris_encoder_rebase_total == 1 but found $val" }

    Write-Output "Smoke test passed: iris_encoder_rebase_total == 1"
    exit 0
} catch {
    Write-Error $_.Exception.Message
    exit 1
} finally {
    if ($proc -and -not $proc.HasExited) {
        Write-Output "Stopping iris-ui (PID $($proc.Id))"
        try { $proc.Kill() } catch { }
    }
}
