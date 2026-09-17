<?php
$a = <<<EOT
  {$x[018]}
  EOT;
$b = <<<EOT
  {$x["\u{zz}"]}
  EOT;
$c = <<<EOT
 x
 EOT;
$d = <<<OUT
{$x[<<<IN
  in
  IN]} {$y[09]}
    OUT;
$e = <<<EOT
  $x[018]
  EOT;
$f = <<<EOT
  {$x[0x8]} {$y[08]} {$z[0]}
      EOT;
$g = <<<EOT
  {$x[`\u{zz}`]}
  EOT;
$h = <<<EOT
  {$x[08]}
  EOT . <<<EOT
    ok
    EOT;
$i = <<<OUT
{$x[<<<IN
  {$y[08]}
  IN]}
    OUT;
$j = <<<EOT
  {$x["a\u{41}b"]}
  EOT;
$k = <<<EOT
  {$x["\u{zz}"]}
EOT;
$l = <<<EOT
  \u{zz}
  EOT;
$m = <<<EOT
   {$x[018]}
   EOT
;
$n = 1;
