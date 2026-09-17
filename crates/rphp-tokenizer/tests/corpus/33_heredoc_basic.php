<?php
$a = <<<EOT
plain
EOT;
$b = <<<EOT
with $var and {$obj->prop} and ${name} and $arr[0] and $arr[key] and $obj->prop and $arr[-1] and $arr[$i]
EOT;
$c = <<<"EOT"
quoted label
EOT;
$d = <<<   EOT
spaces after <<<
EOT;
$e = <<<EOT
EOT;
$f = <<<EOT

EOT;
$g = <<<EOT


EOT;
$h = <<<EOT
EOTX is not the end
EOT is not either
 EOT;
$i = <<<EOT
text
EOT . "concat";
$j = f(<<<EOT
arg
EOT, 1);
$k = [<<<EOT
elem
EOT];
$l = <<<EOT
ends with \$ and \{$ and $ and { and \\
EOT;
$m = <<<EOT
$
EOT;
$n = <<<EOT
{
EOT;
$o = <<<EOT
{$
EOT;
$p = <<<eot
lowercase label
eot;
$q = <<<E_1
underscore digit label
E_1;
$r = <<<EOT
line\
continued
EOT;
$s = <<<EOT
$a[0]$b[-1]$c[$d]$e->f$g?->h${i}{$j}
EOT;
$t = <<<EOT
tab	inside
EOT;
$u = <<<	EOT
tab after <<<
EOT;
$v = <<<EOT
$a[ ]$b[']$c[#]$d[\]
EOT;
$w = <<<EOT
 $a
EOT;
$x = <<<EOT
$a->b->c ${a[1]} {$a[0]}
EOT;
$y = <<<X
x
X;
$z = <<<ÉÜ
utf8 label
ÉÜ;
