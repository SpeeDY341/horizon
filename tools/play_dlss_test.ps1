# Developer test launcher: explicitly opts into DLSS in a fresh disposable home.
param([switch]$Smoke)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$game = Join-Path $root 'target/fast/horizon-client.exe'
$sdk = Join-Path $root '.local/dlss/native'
if (!(Test-Path $game)) { throw 'The DLSS test build is missing. Ask Codex to build this branch.' }
foreach ($dll in @('sl.interposer.dll','sl.common.dll','sl.dlss.dll','nvngx_dlss.dll')) {
    if (!(Test-Path (Join-Path $sdk $dll))) { throw 'The official NVIDIA SDK is missing from .local/dlss/native.' }
}
$resources = Join-Path $root '.local/horizon-client/26.3'
if (!(Test-Path (Join-Path $resources 'horizon-resources.json'))) {
    $versions = Join-Path $env:APPDATA 'horizon-launcher/versions'
    $resources = Get-ChildItem $versions -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | ForEach-Object {
            $candidate = Join-Path $_.FullName '.local/horizon-client/26.3'
            if (Test-Path (Join-Path $candidate 'horizon-resources.json')) { $candidate }
        } | Select-Object -First 1
}
if (!$resources) { throw 'Prepare Minecraft 26.3 once in the Horizon launcher, then run this test again.' }
$scratch = Join-Path ([System.IO.Path]::GetTempPath()) 'horizon-dlss-test'
New-Item -ItemType Directory -Force $scratch | Out-Null
$env:HORIZON_HOME = Join-Path $scratch 'home'
$env:HORIZON_LAUNCHER_HOME = Join-Path $scratch 'launcher'
$env:XDG_DATA_HOME = Join-Path $scratch 'data'
$env:XDG_CONFIG_HOME = Join-Path $scratch 'config'
$env:HORIZON_LAUNCHER_NO_SELF_UPDATE = '1'
$env:HORIZON_NO_AUDIO = '1'
$env:HORIZON_LAUNCHER_TRASH = Join-Path $scratch 'trash'
$env:HORIZON_LAUNCHER_CLIPBOARD = Join-Path $scratch 'clipboard'
$env:HORIZON_LAUNCHER_DESKTOP = Join-Path $scratch 'desktop'
$env:HORIZON_STREAMLINE_SDK = $sdk
Remove-Item Env:HORIZON_DLSS_FORCE_ERROR -ErrorAction SilentlyContinue
Remove-Item Env:HORIZON_GRAPHICS_API -ErrorAction SilentlyContinue
$graphics = Join-Path $scratch 'horizon-graphics.txt'
if (!(Test-Path $graphics)) {
    "graphicsApi:vulkan`ndlss:quality`nfxaa:false`ntaa:false`nupscaling:quality`noverlay:fps`nhdr:false`n" |
        Set-Content $graphics -Encoding ASCII
}
$options = Join-Path $scratch 'options.txt'
if (!(Test-Path $options)) {
    "renderDistance:8`nmaxFps:120`nguiScale:2`n" | Set-Content $options -Encoding ASCII
}
Write-Host 'DLSS test world. Sound is off. Settings in this folder are kept between launches.'
Write-Host 'Esc, Options, Horizon Settings, Advanced Graphics: Anti-aliasing, Upscaler, Quality.'
Write-Host 'Graphics API applies on the next launch. DLSS runs on Vulkan and DirectX 12.'
Write-Host ('Test files and log: ' + $scratch)
Push-Location $root
try {
    $arguments = @('--resources',$resources,'--game-dir',$scratch,'--home',$env:HORIZON_HOME,
        '--world','DLSS-Test','--test-terrain','--creative','--render-distance','8',
        '--width','1280','--height','720')
    if ($Smoke) { $arguments += @('--world-frames','60','--smoke-report',(Join-Path $scratch 'report.json')) }
    # Windows PowerShell 5.1 treats redirected native stderr as ErrorRecords.
    # Keep normal game logging out of PowerShell's terminating-error pipeline.
    # These arguments contain paths (which cannot contain quotes on Windows).
    $quotedArguments = $arguments | ForEach-Object { '"' + $_ + '"' }
    $gameProcess = Start-Process -FilePath $game -ArgumentList $quotedArguments -NoNewWindow `
        -RedirectStandardOutput (Join-Path $scratch 'stdout.log') `
        -RedirectStandardError (Join-Path $scratch 'game.log') -Wait -PassThru
    if ($gameProcess.ExitCode -ne 0) { throw ('Game exited with an error. Log: ' + (Join-Path $scratch 'game.log')) }
} finally { Pop-Location }
