<?php
$a = <<<EOT
    four
      six
    EOT;
$b = <<<EOT
	tab
	EOT;
$c = <<<EOT
  $x
  EOT;
$d = <<<EOT
$x
  EOT;
$e = <<<EOT
  a
   EOT;
$f = <<<EOT
   a
  EOT;
$g = <<<EOT
    {$x}
    EOT;
$h = <<<EOT
  a

  b
  EOT . <<<EOT
    c
  EOT;
$i = <<<EOT
  a
	b
  EOT;
$j = <<<EOT
  ${x}
   EOT;
$k = <<<EOT
    {$x}
  EOT;
$l = <<<EOT

  EOT;
$m = <<<EOT
  
  EOT;
$n = <<<EOT
  a  
  EOT;
$o = <<<EOT
    EOT;
$p = <<<EOT
	EOT;
$q = <<<EOT
  a
  EOT ;
$r = <<<EOT
  a
  EOT
  ;
