# rdp_video.ps1 start|stop: a video playing full screen on a Windows host's desktop, looped, for the RDP probes that
# take a motion script (examples/rdp_link_probe.rs runs it as ZDESK_UAT_MOTION), where tests/rdp_motion.ps1's
# animation is no video. ZDESK_UAT_VIDEO is the file here, copied to the host once; it plays as it is, so make it the
# desktop's size first, or the host's CPU goes on scaling it rather than on its encoder. `start` plays it with ffplay,
# which must be on the host's path, in the desktop session of the user the probe logs on as, through a scheduled task,
# since an SSH session has no desktop; `stop` closes it and removes the task. The host is reached with PowerShell over
# SSH by ZDESK_UAT_TARGET's name, the target in tmp/test_uat.toml, so that name must be an SSH host here as well.
param([Parameter(Mandatory)][ValidateSet('start', 'stop')][string]$Action)
$ErrorActionPreference = 'Stop'

$hostName = $env:ZDESK_UAT_TARGET
if (-not $hostName) { throw 'set ZDESK_UAT_TARGET to the Windows host' }
$dir = 'C:\ci-workspaces\zdesk-video'
$remote = $null

if ($Action -eq 'start') {
    $video = $env:ZDESK_UAT_VIDEO
    if (-not $video -or -not (Test-Path $video)) { throw 'set ZDESK_UAT_VIDEO to the video file to play' }
    $item = Get-Item $video
    $remote = "$dir\$($item.Name)"
    $session = New-PSSession -HostName $hostName -SSHTransport
    try {
        $there = Invoke-Command -Session $session -ArgumentList $dir, $remote -ScriptBlock {
            param($dir, $remote)
            New-Item -ItemType Directory -Force $dir | Out-Null
            if (Test-Path $remote) { (Get-Item $remote).Length } else { -1 }
        }
        if ($there -ne $item.Length) { Copy-Item -Path $item.FullName -Destination $remote -ToSession $session }
    } finally {
        Remove-PSSession $session
    }
}

Invoke-Command -HostName $hostName -SSHTransport -ArgumentList $Action, $remote -ScriptBlock {
    param($Action, $Remote)
    $ErrorActionPreference = 'Stop'
    $task = 'zdesk-video'
    if ($Action -eq 'start') {
        $ffplay = (Get-Command ffplay).Source
        $run = New-ScheduledTaskAction -Execute $ffplay `
            -Argument "-hide_banner -loglevel error -fs -noborder -alwaysontop -an -loop 0 -window_title zdesk-video `"$Remote`""
        # Interactive: in the user's own desktop session, which the probe's logon takes over, and not session 0.
        $who = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive
        Register-ScheduledTask -TaskName $task -Action $run -Principal $who -Force | Out-Null
        Start-ScheduledTask -TaskName $task
        Start-Sleep -Seconds 3
        if (-not (Get-Process ffplay -ErrorAction SilentlyContinue)) {
            throw 'the video did not start: is the user logged on to the desktop?'
        }
        'the video is playing'
    } else {
        Get-Process ffplay -ErrorAction SilentlyContinue | Stop-Process -Force
        Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue
        'the video is stopped'
    }
}
