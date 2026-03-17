Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win {
  [DllImport("user32.dll")] public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
"@

$hwnd = [Win]::FindWindow($null, "Iris")
if ($hwnd -ne [IntPtr]::Zero) {
    [Win]::ShowWindow($hwnd, 9)
    [Win]::SetForegroundWindow($hwnd)
    Write-Host "Brought Iris to front"
} else {
    Write-Host "Iris window not found"
}
