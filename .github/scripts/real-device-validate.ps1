param()

# Real-device validation harness (Windows)
# Starts iris-ui with IRIS_BACKEND=dxgi, captures stdout to a log file,
# waits for capture telemetry, fetches /metrics, and writes artifacts.

try {
    $outdir = "harness-output/phase-6/real-device"
    if (-not (Test-Path $outdir)) { New-Item -ItemType Directory -Path $outdir | Out-Null }

    $logfile = Join-Path $outdir "iris-ui-real-device.log"
    Write-Output "Starting iris-ui (DXGI backend) and writing logs to $logfile"

    # Start iris-ui with stdout/stderr redirected directly to the logfile
    # Set environment for child process then start it with Start-Process
    $env:IRIS_BACKEND = "dxgi"
    Write-Output "Launching iris-ui (cargo run -p iris-ui) with IRIS_BACKEND=dxgi"
    $errfile = "$logfile.err"
    $proc = Start-Process -FilePath "cargo" -ArgumentList "run -p iris-ui --quiet" -RedirectStandardOutput $logfile -RedirectStandardError $errfile -NoNewWindow -PassThru

    # Attempt to bring the Iris UI to the foreground to ensure any
    # device permission prompts are visible and the OS can display UI.
    $bringScript = ".github/scripts/bring-iris-front.ps1"
    if (Test-Path $bringScript) {
        Write-Output "Invoking bring-iris-front helper to restore UI"
        try { & $bringScript } catch { Write-Output "bring-iris-front failed: $_" }
        Start-Sleep -Seconds 2
    }

    # Wait for metrics endpoint to come up
    $metricsUrl = "http://127.0.0.1:9180/metrics"
    $ok = $false
    $deadline = (Get-Date).AddSeconds(40)
    while ((Get-Date) -lt $deadline) {
        try {
            $res = Invoke-WebRequest -Uri $metricsUrl -UseBasicParsing -TimeoutSec 2 -ErrorAction Stop
            if ($res.StatusCode -eq 200) { $ok = $true; break }
        } catch { Start-Sleep -Seconds 1 }
    }

    if (-not $ok) { throw "metrics endpoint did not become available" }

    Write-Output "Metrics available; collecting telemetry for 15 seconds..."
    Start-Sleep -Seconds 15

    # Fetch metrics and save
    $metrics = Invoke-WebRequest -Uri $metricsUrl -UseBasicParsing -TimeoutSec 5 -ErrorAction Stop
    $metricsPath = Join-Path $outdir "metrics.txt"
    $metrics.Content | Out-File -FilePath $metricsPath -Encoding utf8

    # Merge any stderr into the logfile and copy the live log tail into artifacts
    if (Test-Path $errfile) { Get-Content -Path $errfile | Add-Content -Path $logfile }
    Get-Content -Path $logfile -Tail 200 | Out-File -FilePath (Join-Path $outdir "log-tail.txt") -Encoding utf8

    # Basic check: ensure log contains FrameCaptured or frames_captured
    $logText = Get-Content -Path $logfile -Raw -ErrorAction SilentlyContinue
    if ($logText -match "FrameCaptured|frames_captured") {
        Write-Output "Real-device validation observed capture telemetry in logs."
        $result = 0
    } else {
        Write-Error "No capture telemetry observed in logs. Check device availability and permissions."
        $result = 2
    }

    exit $result
} catch {
    Write-Error $_.Exception.Message
    exit 1
} finally {
    if ($proc -and -not $proc.HasExited) {
        Write-Output "Stopping iris-ui (PID $($proc.Id))"
        try { $proc.Kill() } catch { }
    }
}
