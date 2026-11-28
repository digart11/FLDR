# Repeatable warm-launch timing with per-frame diagnostics and message-pump probes.
# Use an isolated copy of the desired Atlas config via -ConfigDirectory.
param(
 [Parameter(Mandatory=$true)][string]$Exe,
 [Parameter(Mandatory=$true)][string]$OutputDirectory,
 [Parameter(Mandatory=$true)][string]$ConfigDirectory,
 [ValidateRange(1,100)][int]$Runs=12,
 [ValidateRange(3,60)][int]$DurationSeconds=6,
 [switch]$FrameLog
)
$ErrorActionPreference='Stop'
if (Get-Process atlas -ErrorAction SilentlyContinue) { throw 'Existing Atlas process; refusing to interfere.' }
$Exe=(Resolve-Path -LiteralPath $Exe).Path
$out=$OutputDirectory
if(Test-Path -LiteralPath $out){throw 'Use a new output directory to preserve captures.'}
New-Item -ItemType Directory -Path $out | Out-Null
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class AtlasProbe {
 [StructLayout(LayoutKind.Sequential)] public struct IO {public ulong Reads,Writes,Other,ReadBytes,WriteBytes,OtherBytes;}
 [DllImport("kernel32.dll",SetLastError=true)] public static extern bool GetProcessIoCounters(IntPtr h,out IO io);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll",SetLastError=true)] public static extern IntPtr SendMessageTimeout(IntPtr h,uint m,UIntPtr w,IntPtr l,uint flags,uint timeout,out UIntPtr result);
 [DllImport("kernel32.dll")] static extern IntPtr OpenThread(uint access,bool inherit,uint id);
 [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr h,out IntPtr text);
 [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr h);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
 public static string ThreadName(uint id) {var h=OpenThread(0x800,false,id);if(h==IntPtr.Zero)return "<access failed>";try{IntPtr p;var hr=GetThreadDescription(h,out p);if(hr<0)return "<description failed>";try{return Marshal.PtrToStringUni(p);}finally{LocalFree(p);}}finally{CloseHandle(h);}}
}
'@
$previousConfig=$env:FLDR_CONFIG_DIR
$previousFrame=$env:GPUI_FRAME_LOG
$env:FLDR_CONFIG_DIR=(Resolve-Path -LiteralPath $ConfigDirectory).Path
$env:GPUI_FRAME_LOG=if($FrameLog){'0'}else{$null}
Get-FileHash -LiteralPath $Exe | ConvertTo-Json | Set-Content (Join-Path $out 'executable.json')
$summary=[Collections.Generic.List[object]]::new()
try {
 for($run=1;$run -le $Runs;$run++) {
  $prefix=Join-Path $out ('run-{0:00}' -f $run)
  $utc=[DateTime]::UtcNow.ToString('o')
  $watch=[Diagnostics.Stopwatch]::StartNew()
  $p=Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe) -WindowStyle Normal -RedirectStandardOutput ($prefix+'.stdout.log') -RedirectStandardError ($prefix+'.stderr.log') -PassThru
  $samples=[Collections.Generic.List[object]]::new()
  $firstVisible=$null; $firstResponse=$null; $nextProbe=0
  while($watch.Elapsed.TotalSeconds -lt $DurationSeconds) {
   $p.Refresh(); if($p.HasExited){throw "Atlas exited unexpectedly: $($p.ExitCode)"}
   $t=$watch.Elapsed.TotalMilliseconds; $hwnd=$p.MainWindowHandle
   $visible=($hwnd -ne [IntPtr]::Zero -and [AtlasProbe]::IsWindowVisible($hwnd))
   if($visible -and $null -eq $firstVisible){$firstVisible=$t}
   $response=$null; $probeMs=$null
   if($visible -and $t -ge $nextProbe){
    $probe=[Diagnostics.Stopwatch]::StartNew(); $result=[UIntPtr]::Zero
    $response=([AtlasProbe]::SendMessageTimeout($hwnd,0,[UIntPtr]::Zero,[IntPtr]::Zero,2,20,[ref]$result) -ne [IntPtr]::Zero)
    $probeMs=$probe.Elapsed.TotalMilliseconds; $nextProbe=$t+250
    if($response -and $null -eq $firstResponse){$firstResponse=$watch.Elapsed.TotalMilliseconds}
   }
   $io=[AtlasProbe+IO]::new(); $ok=[AtlasProbe]::GetProcessIoCounters($p.Handle,[ref]$io)
   $samples.Add([pscustomobject]@{ms=[Math]::Round($t,3);cpu_ms=$p.TotalProcessorTime.TotalMilliseconds;private_bytes=$p.PrivateMemorySize64;working_set=$p.WorkingSet64;threads=$p.Threads.Count;visible=$visible;wm_null_response=$response;probe_ms=$probeMs;io_valid=$ok;read_bytes=$io.ReadBytes;write_bytes=$io.WriteBytes;other_bytes=$io.OtherBytes;read_ops=$io.Reads;write_ops=$io.Writes})
   Start-Sleep -Milliseconds 25
  }
  $samples | Export-Csv -LiteralPath ($prefix+'.samples.csv') -NoTypeInformation
  $p.Threads | ForEach-Object { $reason=$null;try{if($_.ThreadState -eq 'Wait'){$reason=$_.WaitReason.ToString()}}catch{};[pscustomobject]@{id=$_.Id;name=[AtlasProbe]::ThreadName($_.Id);state=$_.ThreadState.ToString();wait_reason=$reason;cpu_ms=$_.TotalProcessorTime.TotalMilliseconds;start_address=$_.StartAddress.ToInt64()} } | Export-Csv -LiteralPath ($prefix+'.threads.csv') -NoTypeInformation
  $closed=$p.CloseMainWindow()
  $exited=$p.WaitForExit(10000)
  if(!$exited){throw "Atlas PID $($p.Id) did not close; left running for inspection."}
  $summary.Add([pscustomobject]@{run=$run;frame_log=[bool]$FrameLog;pid=$p.Id;utc=$utc;first_visible_ms=$firstVisible;first_wm_null_response_ms=$firstResponse;exit_code=$p.ExitCode;samples=$samples.Count;graceful_close_requested=$closed})
  $summary | Export-Csv -LiteralPath (Join-Path $out 'runs.csv') -NoTypeInformation
  Write-Output "Run $run frame_log=$FrameLog visible=$([Math]::Round($firstVisible,1)) response=$([Math]::Round($firstResponse,1)) output=$prefix"
  Start-Sleep -Milliseconds 750
 }
} finally { $env:FLDR_CONFIG_DIR=$previousConfig; $env:GPUI_FRAME_LOG=$previousFrame }
Write-Output "RESULTS=$out"
