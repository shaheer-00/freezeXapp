$ErrorActionPreference = 'Continue'

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class W {
  public delegate bool Proc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(Proc p, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  public struct RECT { public int Left, Top, Right, Bottom; }
  public static IntPtr FindTitle(string want) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => {
      var t = new StringBuilder(256); GetWindowTextW(h, t, 256);
      if (t.ToString() == want) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@

$h = [W]::FindTitle("FreezeGun")
if ($h -eq [IntPtr]::Zero) { Write-Output "FreezeGun window not found"; exit 1 }
Write-Output "hwnd=$h"

# SW_SHOWNA (8): restore without stealing focus from whatever the user is doing.
[W]::ShowWindow($h, 8) | Out-Null
Start-Sleep -Milliseconds 1500

$r = New-Object W+RECT
[W]::GetWindowRect($h, [ref]$r) | Out-Null
$w  = $r.Right - $r.Left
$ht = $r.Bottom - $r.Top

Add-Type -AssemblyName System.Drawing
$bmp = New-Object Drawing.Bitmap $w, $ht
$g = [Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# flags=2 (PW_RENDERFULLCONTENT) is required for WebView2/Chromium surfaces.
$ok = [W]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc)
$bmp.Save("F:\Claude\vibeXcode\Skills\FreezXtime\ui_safe.png")
$g.Dispose(); $bmp.Dispose()

Write-Output "printwindow=$ok size=${w}x${ht}"
