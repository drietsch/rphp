<?php
$a = <<<EOT
x
EOT;$b = 1;
$c = <<<EOT
x
EOT
;
$d = <<<EOT
x
EOT,
$e = <<<EOT
x
EOT)
$f = <<<EOT
x
EOT?>
html
<?php
$g = <<<EOT
x
EOT
EOT;
$h = <<<EOT
$
EOT;
$i = b<<<EOT
binary heredoc
EOT;
$j = B<<<"EOT"
binary quoted heredoc
EOT;
$k = <<<EOT
\
EOT;
$l = <<<EOT
\{$a}
EOT;
$m = <<<EOT
\\{$a}
EOT;
$n = <<<EOT
$a\
EOT;
$o = <<<EOT
  EOT ;
$p = <<<EOT
EOT
EOT;
$q = <<<EOT
 EOT
EOT;
$r = <<<EOT
xEOT
EOT;
$s = <<<EOT
EOT1
EOT_
EOT;
$t = <<<EOT
{$a}
EOT;
$u = <<<EOT
{$a}EOT
EOT;
$v = <<<EOT
$a
EOT;
$w = <<<EOT
${a}
EOT;
$x = <<<EOT
$a[0]
EOT;
$y = <<<EOT
$a->b
EOT;
$z = <<<EOT
EOT ;
$aa = <<<EOT
EOT	;
$bb = <<<E
E
E;
$cc = <<<EOT
{$a}{$b}
{$c}
EOT;
$dd = <<< EOT
x
EOT;
$ee = <<<EOT
x
EOT;$ff = <<<EOT
y
EOT;
$gg = <<<EOT
$a{$b}$c
EOT;
$hh = <<<EOT
a{$b}
EOT;
$ii = <<<EOT
{$b}a
EOT;
$jj = <<<eot
x
EOT;
eot;
$kk = <<<EOT
b<<<EOT
EOT;
$ll = <<<EOT
<<<EOT
EOT;
$mm = <<<EOT
x
	EOT;
