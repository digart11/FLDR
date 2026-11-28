[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$FixtureRoot,
    [string]$BeforeLabel = 'before-final',
    [string]$AfterLabel = 'after-final'
)
$ErrorActionPreference = 'Stop'
if (-not ('FldrThumbnailComparison' -as [type])) {
    Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
public static class FldrThumbnailComparison {
 public static double[] Compare(string a,string b) {
  using(var x=new Bitmap(a)) using(var y=new Bitmap(b)) {
   if(x.Size!=y.Size) throw new Exception("Thumbnail geometry differs");
   double abs=0,squared=0,max=0,alpha=0;
   for(int j=0;j<x.Height;j++) for(int i=0;i<x.Width;i++) {
    Color p=x.GetPixel(i,j),q=y.GetPixel(i,j);
    int[] d={Math.Abs(p.R-q.R),Math.Abs(p.G-q.G),Math.Abs(p.B-q.B)};
    foreach(int n in d){abs+=n;squared+=n*n;max=Math.Max(max,n);}
    if(p.A!=q.A)alpha++;
   }
   double pixels=x.Width*x.Height, mse=squared/(3*pixels);
   return new double[]{abs/(3*pixels),mse==0?Double.PositiveInfinity:10*Math.Log10(255*255/mse),max,alpha};
  }
 }
}
'@
}
foreach ($file in Get-ChildItem -LiteralPath $FixtureRoot -File -Filter "*-$BeforeLabel.png") {
    $stem = $file.Name.Substring(0, $file.Name.Length - ($BeforeLabel.Length + 5))
    $after = Join-Path $file.DirectoryName ($stem + '-' + $AfterLabel + '.png')
    $metrics = [FldrThumbnailComparison]::Compare($file.FullName, $after)
    if ($metrics[3] -ne 0) { throw "Alpha changed: $stem" }
    [pscustomobject]@{ Thumbnail = $stem; RGB_MAE = $metrics[0]; PSNR_dB = $metrics[1]; MaxChannelDifference = $metrics[2]; AlphaMismatches = $metrics[3] }
}
