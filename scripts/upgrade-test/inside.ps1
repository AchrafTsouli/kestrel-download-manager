# Runs inside Windows Sandbox (started by scripts/Test-Upgrade.ps1). Installs Flux 1.0.0, gives it
# two downloads through its browser bridge (the way the extension does), lets one finish and pauses
# the other halfway, then installs the new Kestrel over it and checks that everything carried over.
# Results: C:\results\log.txt and C:\results\done.txt (PASS or FAIL).
$ErrorActionPreference = 'Stop'
$in = 'C:\test'; $out = 'C:\results'
$log = Join-Path $out 'log.txt'
$failures = [System.Collections.Generic.List[string]]::new()
# The host may be reading the log at the same moment: a write is retried, and never stops the test.
function Log($text) {
  $line = '{0:HH:mm:ss} {1}' -f (Get-Date), $text
  for ($i = 0; $i -lt 40; $i++) { try { [IO.File]::AppendAllText($log, $line + "`r`n"); return } catch { Start-Sleep -Milliseconds 50 } }
}
function Check($name, [bool]$ok, $detail = '') { if ($ok) { Log "ok    $name $detail" } else { Log "FAIL  $name $detail"; $failures.Add($name) } }
function WaitFor($what, [scriptblock]$test, $seconds = 60) {
  $until = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $until) { $v = $null; try { $v = & $test } catch {}; if ($v) { return $v }; Start-Sleep -Milliseconds 500 }
  throw "timed out: $what"
}

try {
  Log 'Upgrade test started'
  # ---------------------------------------------------------------- local test server
  $serverOut = Join-Path $env:TEMP 'labserver.txt'
  # Programs run from the sandbox's own disk, not from the mapped folder.
  $lab = Join-Path $env:TEMP 'labserver.exe'; Copy-Item (Join-Path $in 'labserver.exe') $lab
  $server = Start-Process $lab -RedirectStandardOutput $serverOut -RedirectStandardError "$serverOut.err" -WindowStyle Hidden -PassThru
  Start-Sleep 3
  Log ("Server: exited=$($server.HasExited) code=$(if ($server.HasExited) { '0x{0:X}' -f $server.ExitCode }) says: " + ((Get-Content $serverOut, "$serverOut.err" -ErrorAction SilentlyContinue) -join ' | '))
  $base = WaitFor 'the test server' { (Get-Content $serverOut -ErrorAction SilentlyContinue | Select-String 'Lab server on (\S+)').Matches.Groups[1].Value } 20
  $urlA = "$base/upgrade-small.bin?size=3000000&seed=11"
  $urlB = "$base/upgrade-big.bin?size=80000000&seed=12&slow_us=25000"
  Log "Test server: $base"

  # ---------------------------------------------------------------- Flux 1.0.0
  # WaitForExit, not -Wait: Windows PowerShell's -Wait also waits for programs the installer leaves running.
  function Install($file, [string[]]$arguments) {
    Log "Installing $(Split-Path -Leaf $file)"
    $p = Start-Process $file -ArgumentList $arguments -PassThru
    if (!$p.WaitForExit(300000)) { throw "$(Split-Path -Leaf $file) did not finish in 5 minutes" }
    Log "Installer finished (exit code $($p.ExitCode))"
  }
  Install (Join-Path $in 'Flux-Setup-1.0.0-win-x64.exe') '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/TASKS=browserintegration'
  $app = Join-Path $env:LOCALAPPDATA 'Programs\Flux'
  Check 'Flux 1.0.0 installed' (Test-Path (Join-Path $app 'Flux.exe'))
  $manifest = Get-Content (Join-Path $app 'com.swiftfetch.bridge.json') -Raw | ConvertFrom-Json
  $hostExe = $manifest.path; $origin = $manifest.allowed_origins[0]
  Log "1.x bridge: $hostExe for $origin"

  # Clicks "Start Download" in Flux 1.x's dialog whenever it appears (UI Automation, inside the sandbox only).
  $clicker = Start-Job {
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
    $root = [Windows.Automation.AutomationElement]::RootElement
    $cond = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, 'Start Download')
    while ($true) {
      try { foreach ($b in $root.FindAll([Windows.Automation.TreeScope]::Descendants, $cond)) { $b.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke() } } catch {}
      Start-Sleep -Milliseconds 400
    }
  }

  function Bridge($message) {
    # Windows PowerShell opens a child's input with the console encoding; a UTF-8 one would put a byte-order
    # mark in front of the frame, which Chrome never sends.
    [Console]::InputEncoding = New-Object Text.UTF8Encoding $false
    $psi = New-Object Diagnostics.ProcessStartInfo $hostExe, $origin
    $psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true
    $p = [Diagnostics.Process]::Start($psi)
    $body = [Text.Encoding]::UTF8.GetBytes(($message | ConvertTo-Json -Compress -Depth 5))
    $p.StandardInput.BaseStream.Write([BitConverter]::GetBytes([int]$body.Length), 0, 4)
    $p.StandardInput.BaseStream.Write($body, 0, $body.Length); $p.StandardInput.BaseStream.Flush()
    $header = New-Object byte[] 4; [void]$p.StandardOutput.BaseStream.Read($header, 0, 4)
    $answer = New-Object byte[] ([BitConverter]::ToInt32($header, 0)); $read = 0
    while ($read -lt $answer.Length) { $read += $p.StandardOutput.BaseStream.Read($answer, $read, $answer.Length - $read) }
    $p.StandardInput.Close(); $p.WaitForExit(5000) | Out-Null
    [Text.Encoding]::UTF8.GetString($answer) | ConvertFrom-Json
  }
  $downloads = Join-Path $env:USERPROFILE 'Downloads'
  function Found($name) { Get-ChildItem $downloads -Recurse -File -Filter $name -ErrorAction SilentlyContinue | Select-Object -First 1 }

  $r = Bridge @{ action = 'download'; url = $urlA; requestId = [guid]::NewGuid().ToString() }
  Check '1.x accepts a download from the browser bridge' $r.ok ($r | ConvertTo-Json -Compress)
  $fileA = WaitFor 'the small download in 1.x' { $f = Found 'upgrade-small.bin'; if ($f -and $f.Length -eq 3000000) { $f } } 90
  Log "1.x finished $($fileA.FullName)"

  $r = Bridge @{ action = 'download'; url = $urlB; requestId = [guid]::NewGuid().ToString() }
  Check '1.x starts the big download' $r.ok
  Start-Sleep 6
  $r = Bridge @{ action = 'pause'; url = $urlB }
  Check '1.x pauses it' $r.ok
  Start-Sleep 3
  Check 'the big download is not finished in 1.x' (-not (Found 'upgrade-big.bin'))
  $r = Bridge @{ action = 'quit' }
  WaitFor 'Flux 1.x to close' { -not (Get-Process Flux -ErrorAction SilentlyContinue) } 30 | Out-Null
  Stop-Job $clicker; Remove-Job $clicker -Force
  Check '1.x history exists' (Test-Path (Join-Path $env:LOCALAPPDATA 'SwiftFetch\history.db'))

  # ---------------------------------------------------------------- the new Kestrel over it
  # Kestrel needs WebView2 (built into Windows 11 and current Windows 10, but not into Windows Sandbox).
  Install (Join-Path $in 'MicrosoftEdgeWebview2Setup.exe') '/silent', '/install'
  $runtime = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue | Select-Object -First 1
  if (!$runtime.pv) {
    # The sandbox shares Windows' program files with the host but not its registry: the runtime's files
    # can be there while its registration is missing. Register what is there (sandbox only).
    $folder = Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft\EdgeWebView\Application" -Directory -ErrorAction SilentlyContinue | Where-Object { Test-Path (Join-Path $_.FullName 'msedgewebview2.exe') } | Sort-Object { [version]$_.Name } | Select-Object -Last 1
    Log "WebView2 files in the sandbox: $(if ($folder) { $folder.FullName } else { 'none' })"
    if ($folder) {
      $key = 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
      New-Item $key -Force | Out-Null; Set-ItemProperty $key -Name pv -Value $folder.Name; Set-ItemProperty $key -Name name -Value 'Microsoft Edge WebView2 Runtime'
      $runtime = Get-ItemProperty $key
      # The registration alone does not make the shared files usable: point WebView2 at them directly
      # (Microsoft's documented override), for every program this test starts, Kestrel included.
      $env:WEBVIEW2_BROWSER_EXECUTABLE_FOLDER = $folder.FullName
      [Environment]::SetEnvironmentVariable('WEBVIEW2_BROWSER_EXECUTABLE_FOLDER', $folder.FullName, 'User')
    }
  }
  Check 'WebView2 runtime installed' ([bool]$runtime.pv) "$($runtime.pv)"
  if (!$runtime.pv) { foreach ($l in "$env:WINDIR\Temp\msedge_installer.log", "$env:TEMP\msedge_installer.log") { if (Test-Path $l) { Log "--- $l"; Get-Content $l -Tail 25 | ForEach-Object { Log $_ } } } }
  $setup = Get-ChildItem $in -Filter 'Kestrel-Setup-*.exe' | Select-Object -First 1
  Install $setup.FullName '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
  Check 'same folder, new program' (Test-Path (Join-Path $app 'Kestrel.exe'))
  foreach ($old in 'Flux.exe', 'wpfgfx_cor3.dll', 'flux-nmhost.exe') { Check "old $old removed" (-not (Test-Path (Join-Path $app $old))) }
  $cli = Join-Path $app 'kestrel-cli.exe'
  $script:lastAnswer = ''
  # Native error output must not become a PowerShell error here.
  # Started with its output in files: the reliable way in Windows PowerShell 5.1.
  function Invoke-KestrelCli {  # not "Cli": that is a built-in alias of Clear-Item
    $o = Join-Path $env:TEMP ('cli-' + [guid]::NewGuid().ToString('N') + '.txt')
    $p = Start-Process $cli -ArgumentList $args -RedirectStandardOutput $o -RedirectStandardError "$o.err" -PassThru -WindowStyle Hidden
    $p.WaitForExit(60000) | Out-Null
    (Get-Content $o, "$o.err" -ErrorAction SilentlyContinue) -join "`n"
  }
  try { $list = WaitFor 'Kestrel to answer' { $t = Invoke-KestrelCli list; $script:lastAnswer = $t; if ($t -match 'upgrade-') { $t } } 90 }
  catch {
    Log "kestrel-cli said: $script:lastAnswer"
    $o = Join-Path $env:TEMP 'cli-out.txt'
    $p = Start-Process $cli -ArgumentList 'list' -RedirectStandardOutput $o -RedirectStandardError "$o.err" -PassThru -WindowStyle Hidden
    $p.WaitForExit(30000) | Out-Null; $p.Refresh()
    Log ("kestrel-cli run directly: exited=$($p.HasExited) code=0x{0:X} out=[{1}] err=[{2}]" -f $p.ExitCode, ((Get-Content $o -ErrorAction SilentlyContinue) -join ' / '), ((Get-Content "$o.err" -ErrorAction SilentlyContinue) -join ' / '))
    Log "Kestrel processes: $((Get-Process Kestrel, msedgewebview2 -ErrorAction SilentlyContinue | ForEach-Object { $_.Name + '#' + $_.Id }) -join ', ')"
    $data = Join-Path $env:LOCALAPPDATA 'Kestrel'
    Log "Data folder: $((Get-ChildItem $data -Recurse -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName.Substring($data.Length) }) -join ', ')"
    foreach ($f in Get-ChildItem (Join-Path $data 'logs'), (Join-Path $data 'crashes') -File -ErrorAction SilentlyContinue) { Log "--- $($f.Name)"; Get-Content $f.FullName -Tail 30 | ForEach-Object { Log $_ } }
    Log "Windows: $((Get-Process | Where-Object { $_.MainWindowTitle } | ForEach-Object { $_.Name + ': ' + $_.MainWindowTitle }) -join ' | ')"
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    $b = [Windows.Forms.Screen]::PrimaryScreen.Bounds; $bmp = New-Object Drawing.Bitmap $b.Width, $b.Height
    [Drawing.Graphics]::FromImage($bmp).CopyFromScreen($b.Location, [Drawing.Point]::Empty, $b.Size); $bmp.Save((Join-Path $out 'screen.png'))
    throw
  }
  Log "Kestrel's list after the upgrade:`n$list"
  Check 'the finished download came over' ($list -match 'Completed\s+upgrade-small\.bin')
  $paused = [regex]::Match($list, 'Paused\s+upgrade-big\.bin\s+([\d.]+)%')
  Check 'the paused download came over with its progress' ($paused.Success -and [double]$paused.Groups[1].Value -gt 0) $paused.Value
  Check 'data folder is Kestrel' (Test-Path (Join-Path $env:LOCALAPPDATA 'Kestrel\history.db'))
  # The import reads 1.x's history and never changes it (migrate.rs).
  Check '1.x history left in place' (Test-Path (Join-Path $env:LOCALAPPDATA 'SwiftFetch\history.db'))

  Invoke-KestrelCli resume-all | Out-Null
  $done = WaitFor 'the paused download to finish in Kestrel' { $t = Invoke-KestrelCli list; if ($t -match 'Completed\s+upgrade-big\.bin') { $t } } 240
  Log "After resume:`n$done"
  $fileB = Found 'upgrade-big.bin'
  $reference = Join-Path $env:TEMP 'reference.bin'
  (New-Object Net.WebClient).DownloadFile($urlB.Replace('&slow_us=25000', ''), $reference)
  Check 'the resumed file is byte-identical' ((Get-FileHash $fileB.FullName).Hash -eq (Get-FileHash $reference).Hash)
  (New-Object Net.WebClient).DownloadFile($urlA, $reference)
  Check 'the 1.x file is untouched' ((Get-FileHash $fileA.FullName).Hash -eq (Get-FileHash $reference).Hash)

  $chrome = (Get-ItemProperty 'HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.swiftfetch.bridge' -ErrorAction SilentlyContinue).'(default)'
  $bridge = if ($chrome) { (Get-Content $chrome -Raw | ConvertFrom-Json).path } else { '' }
  Check 'the browser bridge now points to kestrel-nmhost.exe' ($bridge -like '*kestrel-nmhost.exe') $bridge
  $programs = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
  Check 'Start menu has Kestrel and no Flux' ((Get-ChildItem $programs -Recurse -Filter 'Kestrel*.lnk') -and -not (Get-ChildItem $programs -Recurse -Filter 'Flux.lnk'))
  Invoke-KestrelCli quit | Out-Null
}
catch { Log "FAIL  $($_.Exception.Message)"; $failures.Add('error') }
finally {
  Set-Content -LiteralPath (Join-Path $out 'done.txt') -Value ($(if ($failures.Count) { 'FAIL: ' + ($failures -join ', ') } else { 'PASS' }))
}
